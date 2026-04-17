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

__device__ __forceinline__ uint16_t makepad_ggml_cuda_f32_to_bf16_bits(float value) {
    const uint32_t bits = __float_as_uint(value);
    return static_cast<uint16_t>(bits >> 16);
}

__device__ __forceinline__ float makepad_ggml_cuda_bf16_round(float value) {
    const uint32_t bits = __float_as_uint(value);
    return __uint_as_float(bits & 0xFFFF0000u);
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

static __global__ void makepad_ggml_cuda_scale_f32_device_f32_index_kernel(
        float * __restrict__ values,
        const float * __restrict__ scales,
        uint32_t scale_index,
        uint32_t n) {
    const float scale = scales[scale_index];
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx >= n) {
        return;
    }
    values[idx] = makepad_ggml_cuda_bf16_round(values[idx] * scale);
}

static __global__ void makepad_ggml_cuda_f32_to_bf16_kernel(
        const float * __restrict__ input,
        uint16_t * __restrict__ output,
        uint32_t n) {
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx >= n) {
        return;
    }
    output[idx] = makepad_ggml_cuda_f32_to_bf16_bits(input[idx]);
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

static __global__ void makepad_ggml_cuda_weighted_sum_rows_f32_kernel(
        const float * __restrict__ batched_inputs,
        const float * __restrict__ weights,
        float * __restrict__ output,
        uint32_t row_count,
        uint32_t input_count) {
    const uint32_t row = blockIdx.x * blockDim.x + threadIdx.x;
    if (row >= row_count) {
        return;
    }
    float total = 0.0f;
    for (uint32_t slot = 0; slot < input_count; ++slot) {
        total += batched_inputs[slot * row_count + row] * weights[slot];
    }
    output[row] = makepad_ggml_cuda_bf16_round(total);
}

static __global__ void makepad_ggml_cuda_mul_f32_kernel(
        const float * __restrict__ left,
        const float * __restrict__ right,
        float * __restrict__ out,
        uint32_t n) {
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx >= n) {
        return;
    }
    out[idx] = makepad_ggml_cuda_bf16_round(left[idx] * right[idx]);
}

static __global__ void makepad_ggml_cuda_gelu_f32_kernel(
        const float * __restrict__ input,
        float * __restrict__ out,
        uint32_t n) {
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx >= n) {
        return;
    }
    const float value = input[idx];
    const float squared = makepad_ggml_cuda_bf16_round(value * value);
    const float cubic = makepad_ggml_cuda_bf16_round(squared * value);
    const float poly = makepad_ggml_cuda_bf16_round(value + makepad_ggml_cuda_bf16_round(0.044715f * cubic));
    const float tanh_input = makepad_ggml_cuda_bf16_round(0.7978846f * poly);
    const float tanh_value = makepad_ggml_cuda_bf16_round(tanhf(tanh_input));
    const float half = makepad_ggml_cuda_bf16_round(0.5f * value);
    out[idx] = makepad_ggml_cuda_bf16_round(half * makepad_ggml_cuda_bf16_round(1.0f + tanh_value));
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

static __global__ void makepad_ggml_cuda_rms_norm_row_weighted_f32_f32weights_kernel(
        const float * __restrict__ input,
        const float * __restrict__ weights_f32,
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
        output[idx] = makepad_ggml_cuda_bf16_round(normalized * weights_f32[idx]);
    }
}

static __global__ void makepad_ggml_cuda_rms_norm_row_weighted_f32_f32weights_precise_kernel(
        const float * __restrict__ input,
        const float * __restrict__ weights_f32,
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
        output[idx] = input[idx] * inv_rms * weights_f32[idx];
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

static __global__ void makepad_ggml_cuda_rms_norm_rows_weighted_f32_f32weights_kernel(
        const float * __restrict__ input,
        const float * __restrict__ weights_f32,
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
        row_out[idx] = makepad_ggml_cuda_bf16_round(normalized * weights_f32[idx]);
    }
}

static __global__ void makepad_ggml_cuda_rms_norm_rows_weighted_f32_f32weights_precise_kernel(
        const float * __restrict__ input,
        const float * __restrict__ weights_f32,
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
        row_out[idx] = row_in[idx] * inv_rms * weights_f32[idx];
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

static __global__ void makepad_ggml_cuda_rms_norm_rows_no_scale_f32_precise_kernel(
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
        row_out[idx] = row_in[idx] * inv_rms;
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
        uint16_t * __restrict__ key_cache,
        uint16_t * __restrict__ value_cache,
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
    key_cache[row_base + slot * head_dim + dim] = makepad_ggml_cuda_f32_to_bf16_bits(keys[idx]);
    value_cache[row_base + dim * max_tokens + slot] = makepad_ggml_cuda_f32_to_bf16_bits(values[idx]);
}

static __global__ void makepad_ggml_cuda_kv_append_f32_device_u32_kernel(
        const float * __restrict__ keys,
        const float * __restrict__ values,
        uint16_t * __restrict__ key_cache,
        uint16_t * __restrict__ value_cache,
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
    key_cache[row_base + slot * head_dim + dim] = makepad_ggml_cuda_f32_to_bf16_bits(keys[idx]);
    value_cache[row_base + dim * max_tokens + slot] = makepad_ggml_cuda_f32_to_bf16_bits(values[idx]);
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
        uint16_t * __restrict__ key_cache,
        uint16_t * __restrict__ value_cache,
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
    const uint32_t cache_head_base = local_row * max_tokens * head_dim;
    const uint32_t cache_key_base = cache_head_base + slot * head_dim;

    for (uint32_t idx = threadIdx.x; idx < head_dim; idx += blockDim.x) {
        if (!is_q && !is_k) {
            value_cache[cache_head_base + idx * max_tokens + slot] =
                makepad_ggml_cuda_f32_to_bf16_bits(row_in[idx] * inv_rms);
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
            key_cache[cache_key_base + idx] = makepad_ggml_cuda_f32_to_bf16_bits(out_value);
        }
    }
}

static __global__ void makepad_ggml_cuda_qkv_norm_rope_cache_f32_device_u32_kernel(
        const float * __restrict__ qkv,
        const uint16_t * __restrict__ q_weights_bf16,
        const uint16_t * __restrict__ k_weights_bf16,
        float * __restrict__ q_out,
        uint16_t * __restrict__ key_cache,
        uint16_t * __restrict__ value_cache,
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
    if (max_tokens == 0) {
        return;
    }
    const uint32_t slot = position % max_tokens;

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
    const uint32_t cache_head_base = local_row * max_tokens * head_dim;
    const uint32_t cache_key_base = cache_head_base + slot * head_dim;

    for (uint32_t idx = threadIdx.x; idx < head_dim; idx += blockDim.x) {
        if (!is_q && !is_k) {
            value_cache[cache_head_base + idx * max_tokens + slot] =
                makepad_ggml_cuda_f32_to_bf16_bits(row_in[idx] * inv_rms);
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
            key_cache[cache_key_base + idx] = makepad_ggml_cuda_f32_to_bf16_bits(out_value);
        }
    }
}

static __global__ void makepad_ggml_cuda_qkv_norm_rope_cache_rows_f32_kernel(
        const float * __restrict__ qkv,
        const uint16_t * __restrict__ q_weights_bf16,
        const uint16_t * __restrict__ k_weights_bf16,
        float * __restrict__ q_out,
        uint16_t * __restrict__ key_cache,
        uint16_t * __restrict__ value_cache,
        uint32_t q_head_count,
        uint32_t k_head_count,
        uint32_t head_dim,
        uint32_t qkv_row_stride,
        uint32_t q_out_row_stride,
        uint32_t q_offset,
        uint32_t k_offset,
        uint32_t v_offset,
        uint32_t rotary_dim,
        float base,
        uint32_t start_position,
        float eps,
        uint32_t max_tokens,
        uint32_t start_slot,
        uint32_t row_count) {
    const uint32_t rows_per_token = q_head_count + 2u * k_head_count;
    const uint32_t row_index = blockIdx.x;
    if (row_index >= row_count * rows_per_token) {
        return;
    }

    const uint32_t token_idx = row_index / rows_per_token;
    const uint32_t row = row_index % rows_per_token;
    const uint32_t position = start_position + token_idx;
    const uint32_t slot = (start_slot + token_idx) % max_tokens;

    const bool is_q = row < q_head_count;
    const bool is_k = row >= q_head_count && row < q_head_count + k_head_count;
    const uint32_t local_row = is_q ? row : (row - q_head_count) % k_head_count;
    const uint32_t source_offset = is_q ? q_offset : (is_k ? k_offset : v_offset);
    const float * row_in = qkv + token_idx * qkv_row_stride + source_offset + local_row * head_dim;

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
    const uint32_t cache_head_base = local_row * max_tokens * head_dim;
    const uint32_t cache_key_base = cache_head_base + slot * head_dim;

    for (uint32_t idx = threadIdx.x; idx < head_dim; idx += blockDim.x) {
        if (!is_q && !is_k) {
            value_cache[cache_head_base + idx * max_tokens + slot] =
                makepad_ggml_cuda_f32_to_bf16_bits(row_in[idx] * inv_rms);
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
            q_out[token_idx * q_out_row_stride + local_row * head_dim + idx] = out_value;
        } else {
            key_cache[cache_key_base + idx] = makepad_ggml_cuda_f32_to_bf16_bits(out_value);
        }
    }
}

static __global__ void makepad_ggml_cuda_qkv_norm_rope_cache_rows_f32_device_u32_kernel(
        const float * __restrict__ qkv,
        const uint16_t * __restrict__ q_weights_bf16,
        const uint16_t * __restrict__ k_weights_bf16,
        float * __restrict__ q_out,
        uint16_t * __restrict__ key_cache,
        uint16_t * __restrict__ value_cache,
        uint32_t q_head_count,
        uint32_t k_head_count,
        uint32_t head_dim,
        uint32_t qkv_row_stride,
        uint32_t q_out_row_stride,
        uint32_t q_offset,
        uint32_t k_offset,
        uint32_t v_offset,
        uint32_t rotary_dim,
        float base,
        const uint32_t * __restrict__ start_position_device_u32,
        float eps,
        uint32_t max_tokens,
        const uint32_t * __restrict__ start_slot_device_u32,
        uint32_t row_count) {
    const uint32_t start_position = *start_position_device_u32;
    const uint32_t start_slot = *start_slot_device_u32;
    const uint32_t rows_per_token = q_head_count + 2u * k_head_count;
    const uint32_t row_index = blockIdx.x;
    if (row_index >= row_count * rows_per_token) {
        return;
    }

    const uint32_t token_idx = row_index / rows_per_token;
    const uint32_t row = row_index % rows_per_token;
    const uint32_t position = start_position + token_idx;
    const uint32_t slot = (start_slot + token_idx) % max_tokens;

    const bool is_q = row < q_head_count;
    const bool is_k = row >= q_head_count && row < q_head_count + k_head_count;
    const uint32_t local_row = is_q ? row : (row - q_head_count) % k_head_count;
    const uint32_t source_offset = is_q ? q_offset : (is_k ? k_offset : v_offset);
    const float * row_in = qkv + token_idx * qkv_row_stride + source_offset + local_row * head_dim;

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
    const uint32_t cache_head_base = local_row * max_tokens * head_dim;
    const uint32_t cache_key_base = cache_head_base + slot * head_dim;

    for (uint32_t idx = threadIdx.x; idx < head_dim; idx += blockDim.x) {
        if (!is_q && !is_k) {
            value_cache[cache_head_base + idx * max_tokens + slot] =
                makepad_ggml_cuda_f32_to_bf16_bits(row_in[idx] * inv_rms);
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
            q_out[token_idx * q_out_row_stride + local_row * head_dim + idx] = out_value;
        } else {
            key_cache[cache_key_base + idx] = makepad_ggml_cuda_f32_to_bf16_bits(out_value);
        }
    }
}

static __global__ void makepad_ggml_cuda_attention_logits_seq_f32_kernel(
        const float * __restrict__ q,
        const uint16_t * __restrict__ key_cache,
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
    const uint16_t * k_row = key_cache + kv_head * kv_row_stride + slot * head_dim;
    float sum = 0.0f;
    for (uint32_t dim = threadIdx.x; dim < head_dim; dim += blockDim.x) {
        sum += q_row[dim] * makepad_ggml_cuda_bf16_to_f32(k_row[dim]);
    }
    sum = makepad_ggml_cuda_block_reduce_sum(sum);
    if (threadIdx.x == 0) {
        logits[q_head * logits_row_stride + token] = makepad_ggml_cuda_bf16_round(sum);
    }
}

template <uint32_t tokens_per_block>
static __global__ void makepad_ggml_cuda_attention_logits_seq_f32_device_u32_kernel(
        const float * __restrict__ q,
        const uint16_t * __restrict__ key_cache,
        float * __restrict__ logits,
        uint32_t q_head_count,
        uint32_t q_heads_per_kv,
        uint32_t head_dim,
        uint32_t kv_row_stride,
        const uint32_t * __restrict__ seq_len_device_u32,
        const uint32_t * __restrict__ start_slot_device_u32,
        uint32_t capacity,
        uint32_t logits_row_stride) {
    const uint32_t q_head = blockIdx.x;
    if (q_head >= q_head_count) {
        return;
    }
    const uint32_t seq_len = *seq_len_device_u32;
    const uint32_t start_slot = *start_slot_device_u32;
    const uint32_t token_base = blockIdx.y * tokens_per_block;
    if (token_base >= seq_len) {
        return;
    }
    const uint32_t kv_head = q_head / q_heads_per_kv;
    const float * q_row = q + q_head * head_dim;
    const uint16_t * key_row = key_cache + kv_head * kv_row_stride;
    const bool cache_q = head_dim <= blockDim.x;
    __shared__ float shared_q[256];
    if (cache_q && threadIdx.x < head_dim) {
        shared_q[threadIdx.x] = q_row[threadIdx.x];
    }
    __syncthreads();

    for (uint32_t token_offset = 0; token_offset < tokens_per_block; ++token_offset) {
        const uint32_t token = token_base + token_offset;
        if (token >= seq_len) {
            break;
        }
        const uint32_t slot = (start_slot + token) % capacity;
        const uint16_t * k_row = key_row + slot * head_dim;
        float sum = 0.0f;
        if (cache_q) {
            if (threadIdx.x < head_dim) {
                sum = shared_q[threadIdx.x] * makepad_ggml_cuda_bf16_to_f32(k_row[threadIdx.x]);
            }
        } else {
            for (uint32_t dim = threadIdx.x; dim < head_dim; dim += blockDim.x) {
                sum += q_row[dim] * makepad_ggml_cuda_bf16_to_f32(k_row[dim]);
            }
        }
        sum = makepad_ggml_cuda_block_reduce_sum(sum);
        if (threadIdx.x == 0) {
            logits[q_head * logits_row_stride + token] = makepad_ggml_cuda_bf16_round(sum);
        }
        __syncthreads();
    }
}

template <uint32_t tokens_per_block>
static __global__ void makepad_ggml_cuda_attention_logits_seq_f32_device_u32_vec2_kernel(
        const float * __restrict__ q,
        const uint16_t * __restrict__ key_cache,
        float * __restrict__ logits,
        uint32_t q_head_count,
        uint32_t q_heads_per_kv,
        uint32_t head_dim,
        uint32_t kv_row_stride,
        const uint32_t * __restrict__ seq_len_device_u32,
        const uint32_t * __restrict__ start_slot_device_u32,
        uint32_t capacity,
        uint32_t logits_row_stride) {
    const uint32_t q_head = blockIdx.x;
    if (q_head >= q_head_count) {
        return;
    }
    const uint32_t seq_len = *seq_len_device_u32;
    const uint32_t start_slot = *start_slot_device_u32;
    const uint32_t token_base = blockIdx.y * tokens_per_block;
    if (token_base >= seq_len) {
        return;
    }

    const uint32_t kv_head = q_head / q_heads_per_kv;
    const float2 * q_row = reinterpret_cast<const float2 *>(q + q_head * head_dim);
    const uint16_t * key_row = key_cache + kv_head * kv_row_stride;
    const uint32_t head_dim_pairs = head_dim >> 1;

    for (uint32_t token_offset = 0; token_offset < tokens_per_block; ++token_offset) {
        const uint32_t token = token_base + token_offset;
        if (token >= seq_len) {
            break;
        }
        const uint32_t slot = (start_slot + token) % capacity;
        const uint32_t * k_row_u32 = reinterpret_cast<const uint32_t *>(key_row + slot * head_dim);
        float sum = 0.0f;
        for (uint32_t pair = threadIdx.x; pair < head_dim_pairs; pair += blockDim.x) {
            const float2 qv = q_row[pair];
            const uint32_t packed = k_row_u32[pair];
            const float k0 = makepad_ggml_cuda_bf16_to_f32(static_cast<uint16_t>(packed));
            const float k1 = makepad_ggml_cuda_bf16_to_f32(static_cast<uint16_t>(packed >> 16));
            sum += qv.x * k0 + qv.y * k1;
        }
        sum = makepad_ggml_cuda_block_reduce_sum(sum);
        if (threadIdx.x == 0) {
            logits[q_head * logits_row_stride + token] = makepad_ggml_cuda_bf16_round(sum);
        }
        __syncthreads();
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

static __global__ void makepad_ggml_cuda_softmax_rows_causal_f32_kernel(
        float * __restrict__ logits,
        uint32_t query_count,
        uint32_t row_count,
        uint32_t row_stride,
        uint32_t base_seq_len,
        uint32_t max_seq_len) {
    const uint32_t row = blockIdx.x;
    if (row >= row_count) {
        return;
    }
    const uint32_t query_idx = row % query_count;
    const uint32_t seq_len = min(base_seq_len + query_idx + 1u, max_seq_len);
    float * row_values = logits + row * row_stride;

    float max_value = -CUDART_INF_F;
    for (uint32_t idx = threadIdx.x; idx < seq_len; idx += blockDim.x) {
        const float value = makepad_ggml_cuda_bf16_round(row_values[idx]);
        row_values[idx] = value;
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
        sum += expf(row_values[idx] - shared_max);
    }
    sum = makepad_ggml_cuda_block_reduce_sum(sum);
    if (threadIdx.x == 0) {
        shared_sum = sum;
    }
    __syncthreads();

    for (uint32_t idx = threadIdx.x; idx < max_seq_len; idx += blockDim.x) {
        row_values[idx] = idx < seq_len
            ? makepad_ggml_cuda_bf16_round(expf(row_values[idx] - shared_max) / shared_sum)
            : 0.0f;
    }
}

static __global__ void makepad_ggml_cuda_softmax_rows_causal_f32_device_u32_kernel(
        float * __restrict__ logits,
        uint32_t query_count,
        uint32_t row_count,
        uint32_t row_stride,
        const uint32_t * __restrict__ base_seq_len_device_u32,
        uint32_t max_seq_len) {
    const uint32_t base_seq_len = *base_seq_len_device_u32;
    const uint32_t row = blockIdx.x;
    if (row >= row_count) {
        return;
    }
    const uint32_t query_idx = row % query_count;
    const uint32_t seq_len = min(base_seq_len + query_idx + 1u, max_seq_len);
    float * row_values = logits + row * row_stride;

    float max_value = -CUDART_INF_F;
    for (uint32_t idx = threadIdx.x; idx < seq_len; idx += blockDim.x) {
        const float value = makepad_ggml_cuda_bf16_round(row_values[idx]);
        row_values[idx] = value;
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
        sum += expf(row_values[idx] - shared_max);
    }
    sum = makepad_ggml_cuda_block_reduce_sum(sum);
    if (threadIdx.x == 0) {
        shared_sum = sum;
    }
    __syncthreads();

    for (uint32_t idx = threadIdx.x; idx < max_seq_len; idx += blockDim.x) {
        row_values[idx] = idx < seq_len
            ? makepad_ggml_cuda_bf16_round(expf(row_values[idx] - shared_max) / shared_sum)
            : 0.0f;
    }
}

static __global__ void makepad_ggml_cuda_softmax_rows_causal_bf16_kernel(
        const float * __restrict__ logits,
        uint16_t * __restrict__ probs,
        uint32_t query_count,
        uint32_t row_count,
        uint32_t row_stride,
        uint32_t base_seq_len,
        uint32_t max_seq_len) {
    const uint32_t row = blockIdx.x;
    if (row >= row_count) {
        return;
    }
    const uint32_t query_idx = row % query_count;
    const uint32_t seq_len = min(base_seq_len + query_idx + 1u, max_seq_len);
    const float * row_logits = logits + row * row_stride;
    uint16_t * row_probs = probs + row * row_stride;

    float max_value = -CUDART_INF_F;
    for (uint32_t idx = threadIdx.x; idx < seq_len; idx += blockDim.x) {
        const float value = makepad_ggml_cuda_bf16_round(row_logits[idx]);
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
        sum += expf(makepad_ggml_cuda_bf16_round(row_logits[idx]) - shared_max);
    }
    sum = makepad_ggml_cuda_block_reduce_sum(sum);
    if (threadIdx.x == 0) {
        shared_sum = sum;
    }
    __syncthreads();

    for (uint32_t idx = threadIdx.x; idx < max_seq_len; idx += blockDim.x) {
        const float value = idx < seq_len
            ? expf(makepad_ggml_cuda_bf16_round(row_logits[idx]) - shared_max) / shared_sum
            : 0.0f;
        row_probs[idx] = makepad_ggml_cuda_f32_to_bf16_bits(value);
    }
}

static __global__ void makepad_ggml_cuda_softmax_rows_causal_bf16_device_u32_kernel(
        const float * __restrict__ logits,
        uint16_t * __restrict__ probs,
        uint32_t query_count,
        uint32_t row_count,
        uint32_t row_stride,
        const uint32_t * __restrict__ base_seq_len_device_u32,
        uint32_t max_seq_len) {
    const uint32_t base_seq_len = *base_seq_len_device_u32;
    const uint32_t row = blockIdx.x;
    if (row >= row_count) {
        return;
    }
    const uint32_t query_idx = row % query_count;
    const uint32_t seq_len = min(base_seq_len + query_idx + 1u, max_seq_len);
    const float * row_logits = logits + row * row_stride;
    uint16_t * row_probs = probs + row * row_stride;

    float max_value = -CUDART_INF_F;
    for (uint32_t idx = threadIdx.x; idx < seq_len; idx += blockDim.x) {
        const float value = makepad_ggml_cuda_bf16_round(row_logits[idx]);
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
        sum += expf(makepad_ggml_cuda_bf16_round(row_logits[idx]) - shared_max);
    }
    sum = makepad_ggml_cuda_block_reduce_sum(sum);
    if (threadIdx.x == 0) {
        shared_sum = sum;
    }
    __syncthreads();

    for (uint32_t idx = threadIdx.x; idx < max_seq_len; idx += blockDim.x) {
        const float value = idx < seq_len
            ? expf(makepad_ggml_cuda_bf16_round(row_logits[idx]) - shared_max) / shared_sum
            : 0.0f;
        row_probs[idx] = makepad_ggml_cuda_f32_to_bf16_bits(value);
    }
}

static __global__ void makepad_ggml_cuda_softmax_rows_causal_vision_bf16_kernel(
        const float * __restrict__ logits,
        uint16_t * __restrict__ probs,
        uint32_t query_count,
        uint32_t row_count,
        uint32_t row_stride,
        uint32_t base_seq_len,
        uint32_t max_seq_len,
        uint32_t chunk_start_position,
        uint32_t vision_start_position,
        uint32_t vision_end_position) {
    const uint32_t row = blockIdx.x;
    if (row >= row_count) {
        return;
    }
    const uint32_t query_idx = row % query_count;
    const uint32_t query_position = chunk_start_position + query_idx;
    const bool query_is_vision =
        query_position >= vision_start_position && query_position <= vision_end_position;
    const uint32_t causal_seq_len = min(base_seq_len + query_idx + 1u, max_seq_len);
    const float * row_logits = logits + row * row_stride;
    uint16_t * row_probs = probs + row * row_stride;

    float max_value = -CUDART_INF_F;
    for (uint32_t idx = threadIdx.x; idx < max_seq_len; idx += blockDim.x) {
        const bool same_vision_group =
            query_is_vision && idx >= vision_start_position && idx <= vision_end_position;
        const bool allowed = idx < causal_seq_len || same_vision_group;
        if (allowed) {
            const float value = makepad_ggml_cuda_bf16_round(row_logits[idx]);
            max_value = value > max_value ? value : max_value;
        }
    }
    max_value = makepad_ggml_cuda_block_reduce_max(max_value);
    __shared__ float shared_max;
    __shared__ float shared_sum;
    if (threadIdx.x == 0) {
        shared_max = max_value;
    }
    __syncthreads();

    float sum = 0.0f;
    for (uint32_t idx = threadIdx.x; idx < max_seq_len; idx += blockDim.x) {
        const bool same_vision_group =
            query_is_vision && idx >= vision_start_position && idx <= vision_end_position;
        const bool allowed = idx < causal_seq_len || same_vision_group;
        if (allowed) {
            sum += expf(makepad_ggml_cuda_bf16_round(row_logits[idx]) - shared_max);
        }
    }
    sum = makepad_ggml_cuda_block_reduce_sum(sum);
    if (threadIdx.x == 0) {
        shared_sum = sum;
    }
    __syncthreads();

    for (uint32_t idx = threadIdx.x; idx < max_seq_len; idx += blockDim.x) {
        const bool same_vision_group =
            query_is_vision && idx >= vision_start_position && idx <= vision_end_position;
        const bool allowed = idx < causal_seq_len || same_vision_group;
        const float value = allowed
            ? expf(makepad_ggml_cuda_bf16_round(row_logits[idx]) - shared_max) / shared_sum
            : 0.0f;
        row_probs[idx] = makepad_ggml_cuda_f32_to_bf16_bits(value);
    }
}

static __global__ void makepad_ggml_cuda_attention_weighted_sum_f32_kernel(
        const float * __restrict__ probs,
        const uint16_t * __restrict__ value_cache,
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
    const uint16_t * value_row = value_cache + kv_head * kv_row_stride;
    float acc = 0.0f;
    for (uint32_t token = 0; token < seq_len; ++token) {
        const uint32_t slot = (start_slot + token) % capacity;
        const float value = makepad_ggml_cuda_bf16_to_f32(value_row[dim * capacity + slot]);
        acc += row_probs[token] * value;
    }
    out[q_head * out_row_stride + dim] = makepad_ggml_cuda_bf16_round(acc);
}

static __global__ void makepad_ggml_cuda_attention_weighted_sum_f32_device_u32_kernel(
        const float * __restrict__ probs,
        const uint16_t * __restrict__ value_cache,
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
    const uint16_t * value_row = value_cache + kv_head * kv_row_stride;
    float acc = 0.0f;
    for (uint32_t token = 0; token < seq_len; ++token) {
        const float value = makepad_ggml_cuda_bf16_to_f32(value_row[dim * capacity + token]);
        acc += row_probs[token] * value;
    }
    out[q_head * out_row_stride + dim] = makepad_ggml_cuda_bf16_round(acc);
}

static __global__ void makepad_ggml_cuda_attention_softmax_weighted_sum_f32_kernel(
        const float * __restrict__ logits,
        const uint16_t * __restrict__ value_cache,
        float * __restrict__ out,
        uint32_t q_head_count,
        uint32_t q_heads_per_kv,
        uint32_t head_dim,
        uint32_t kv_row_stride,
        uint32_t seq_len,
        uint32_t start_slot,
        uint32_t capacity,
        uint32_t logits_row_stride,
        uint32_t out_row_stride) {
    extern __shared__ float shared_probs[];
    const uint32_t q_head = blockIdx.y;
    const uint32_t dim = blockIdx.x * blockDim.x + threadIdx.x;
    if (q_head >= q_head_count) {
        return;
    }
    const bool valid_dim = dim < head_dim;

    const float * row_logits = logits + q_head * logits_row_stride;
    float max_value = -CUDART_INF_F;
    for (uint32_t token = threadIdx.x; token < seq_len; token += blockDim.x) {
        const float value = row_logits[token];
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
    for (uint32_t token = threadIdx.x; token < seq_len; token += blockDim.x) {
        sum += expf(row_logits[token] - shared_max);
    }
    sum = makepad_ggml_cuda_block_reduce_sum(sum);
    if (threadIdx.x == 0) {
        shared_sum = sum;
    }
    __syncthreads();

    for (uint32_t token = threadIdx.x; token < seq_len; token += blockDim.x) {
        shared_probs[token] = makepad_ggml_cuda_bf16_round(expf(row_logits[token] - shared_max) / shared_sum);
    }
    __syncthreads();

    if (valid_dim) {
        const uint32_t kv_head = q_head / q_heads_per_kv;
        const uint16_t * value_row = value_cache + kv_head * kv_row_stride;
        float acc = 0.0f;
        for (uint32_t token = 0; token < seq_len; ++token) {
            const float prob = shared_probs[token];
            const uint32_t slot = (start_slot + token) % capacity;
            const float value = makepad_ggml_cuda_bf16_to_f32(value_row[dim * capacity + slot]);
            acc += prob * value;
        }
        out[q_head * out_row_stride + dim] = makepad_ggml_cuda_bf16_round(acc);
    }
}

static __global__ void makepad_ggml_cuda_attention_softmax_weighted_sum_f32_device_u32_kernel(
        const float * __restrict__ logits,
        const uint16_t * __restrict__ value_cache,
        float * __restrict__ out,
        uint32_t q_head_count,
        uint32_t q_heads_per_kv,
        uint32_t head_dim,
        uint32_t kv_row_stride,
        const uint32_t * __restrict__ seq_len_device_u32,
        const uint32_t * __restrict__ start_slot_device_u32,
        uint32_t capacity,
        uint32_t logits_row_stride,
        uint32_t out_row_stride) {
    extern __shared__ float shared_probs[];
    const uint32_t seq_len = *seq_len_device_u32;
    const uint32_t q_head = blockIdx.y;
    const uint32_t dim = blockIdx.x * blockDim.x + threadIdx.x;
    if (q_head >= q_head_count) {
        return;
    }
    const bool valid_dim = dim < head_dim;
    const uint32_t start_slot = *start_slot_device_u32;

    const float * row_logits = logits + q_head * logits_row_stride;
    float max_value = -CUDART_INF_F;
    for (uint32_t token = threadIdx.x; token < seq_len; token += blockDim.x) {
        const float value = row_logits[token];
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
    for (uint32_t token = threadIdx.x; token < seq_len; token += blockDim.x) {
        sum += expf(row_logits[token] - shared_max);
    }
    sum = makepad_ggml_cuda_block_reduce_sum(sum);
    if (threadIdx.x == 0) {
        shared_sum = sum;
    }
    __syncthreads();

    for (uint32_t token = threadIdx.x; token < seq_len; token += blockDim.x) {
        shared_probs[token] = makepad_ggml_cuda_bf16_round(expf(row_logits[token] - shared_max) / shared_sum);
    }
    __syncthreads();

    if (valid_dim) {
        const uint32_t kv_head = q_head / q_heads_per_kv;
        const uint16_t * value_row = value_cache + kv_head * kv_row_stride;
        float acc = 0.0f;
        for (uint32_t token = 0; token < seq_len; ++token) {
            const float prob = shared_probs[token];
            const uint32_t slot = (start_slot + token) % capacity;
            const float value = makepad_ggml_cuda_bf16_to_f32(value_row[dim * capacity + slot]);
            acc += prob * value;
        }
        out[q_head * out_row_stride + dim] = makepad_ggml_cuda_bf16_round(acc);
    }
}

static __global__ void makepad_ggml_cuda_attention_softmax_weighted_sum_pair_f32_device_u32_kernel(
        const float * __restrict__ logits,
        const uint16_t * __restrict__ value_cache,
        float * __restrict__ out,
        uint32_t kv_head_count,
        uint32_t head_dim,
        uint32_t kv_row_stride,
        const uint32_t * __restrict__ seq_len_device_u32,
        const uint32_t * __restrict__ start_slot_device_u32,
        uint32_t capacity,
        uint32_t logits_row_stride,
        uint32_t out_row_stride) {
    extern __shared__ float shared_probs[];
    float * shared_probs_0 = shared_probs;
    float * shared_probs_1 = shared_probs + capacity;

    const uint32_t seq_len = *seq_len_device_u32;
    const uint32_t start_slot = *start_slot_device_u32;
    const uint32_t kv_head = blockIdx.y;
    const uint32_t dim = blockIdx.x * blockDim.x + threadIdx.x;
    if (kv_head >= kv_head_count) {
        return;
    }
    const bool valid_dim = dim < head_dim;
    const uint32_t q_head_0 = kv_head * 2;
    const uint32_t q_head_1 = q_head_0 + 1;
    const float * row_logits_0 = logits + q_head_0 * logits_row_stride;
    const float * row_logits_1 = logits + q_head_1 * logits_row_stride;

    float max_value_0 = -CUDART_INF_F;
    float max_value_1 = -CUDART_INF_F;
    for (uint32_t token = threadIdx.x; token < seq_len; token += blockDim.x) {
        const float value_0 = row_logits_0[token];
        const float value_1 = row_logits_1[token];
        max_value_0 = value_0 > max_value_0 ? value_0 : max_value_0;
        max_value_1 = value_1 > max_value_1 ? value_1 : max_value_1;
    }
    max_value_0 = makepad_ggml_cuda_block_reduce_max(max_value_0);
    max_value_1 = makepad_ggml_cuda_block_reduce_max(max_value_1);

    __shared__ float shared_max_0;
    __shared__ float shared_max_1;
    __shared__ float shared_sum_0;
    __shared__ float shared_sum_1;
    if (threadIdx.x == 0) {
        shared_max_0 = max_value_0;
        shared_max_1 = max_value_1;
    }
    __syncthreads();

    float sum_0 = 0.0f;
    float sum_1 = 0.0f;
    for (uint32_t token = threadIdx.x; token < seq_len; token += blockDim.x) {
        sum_0 += expf(row_logits_0[token] - shared_max_0);
        sum_1 += expf(row_logits_1[token] - shared_max_1);
    }
    sum_0 = makepad_ggml_cuda_block_reduce_sum(sum_0);
    sum_1 = makepad_ggml_cuda_block_reduce_sum(sum_1);
    if (threadIdx.x == 0) {
        shared_sum_0 = sum_0;
        shared_sum_1 = sum_1;
    }
    __syncthreads();

    for (uint32_t token = threadIdx.x; token < seq_len; token += blockDim.x) {
        shared_probs_0[token] = makepad_ggml_cuda_bf16_round(expf(row_logits_0[token] - shared_max_0) / shared_sum_0);
        shared_probs_1[token] = makepad_ggml_cuda_bf16_round(expf(row_logits_1[token] - shared_max_1) / shared_sum_1);
    }
    __syncthreads();

    if (valid_dim) {
        const uint16_t * value_row = value_cache + kv_head * kv_row_stride;
        float acc_0 = 0.0f;
        float acc_1 = 0.0f;
        for (uint32_t token = 0; token < seq_len; ++token) {
            const uint32_t slot = (start_slot + token) % capacity;
            const float value = makepad_ggml_cuda_bf16_to_f32(value_row[dim * capacity + slot]);
            acc_0 += shared_probs_0[token] * value;
            acc_1 += shared_probs_1[token] * value;
        }
        out[q_head_0 * out_row_stride + dim] = makepad_ggml_cuda_bf16_round(acc_0);
        out[q_head_1 * out_row_stride + dim] = makepad_ggml_cuda_bf16_round(acc_1);
    }
}

static __global__ void makepad_ggml_cuda_attention_weighted_sum_transposed_pair_f32_device_u32_kernel(
        const float * __restrict__ probs,
        const uint16_t * __restrict__ value_cache,
        float * __restrict__ out,
        uint32_t kv_head_count,
        uint32_t head_dim,
        uint32_t kv_row_stride,
        const uint32_t * __restrict__ seq_len_device_u32,
        const uint32_t * __restrict__ start_slot_device_u32,
        uint32_t capacity,
        uint32_t probs_row_stride,
        uint32_t out_row_stride) {
    const uint32_t dim = blockIdx.x;
    const uint32_t kv_head = blockIdx.y;
    if (dim >= head_dim || kv_head >= kv_head_count) {
        return;
    }

    const uint32_t seq_len = *seq_len_device_u32;
    const uint32_t start_slot = *start_slot_device_u32;
    const uint32_t q_head_0 = kv_head * 2;
    const uint32_t q_head_1 = q_head_0 + 1;
    const float * row_probs_0 = probs + q_head_0 * probs_row_stride;
    const float * row_probs_1 = probs + q_head_1 * probs_row_stride;
    const uint16_t * value_row = value_cache + kv_head * kv_row_stride + dim * capacity;

    float acc_0 = 0.0f;
    float acc_1 = 0.0f;
    for (uint32_t token = threadIdx.x; token < seq_len; token += blockDim.x) {
        const uint32_t slot = (start_slot + token) % capacity;
        const float value = makepad_ggml_cuda_bf16_to_f32(value_row[slot]);
        acc_0 += row_probs_0[token] * value;
        acc_1 += row_probs_1[token] * value;
    }

    acc_0 = makepad_ggml_cuda_block_reduce_sum(acc_0);
    __syncthreads();
    acc_1 = makepad_ggml_cuda_block_reduce_sum(acc_1);
    __syncthreads();
    if (threadIdx.x == 0) {
        out[q_head_0 * out_row_stride + dim] = makepad_ggml_cuda_bf16_round(acc_0);
        out[q_head_1 * out_row_stride + dim] = makepad_ggml_cuda_bf16_round(acc_1);
    }
}

static __global__ void makepad_ggml_cuda_attention_weighted_sum_transposed_group8_f32_device_u32_kernel(
        const float * __restrict__ probs,
        const uint16_t * __restrict__ value_cache,
        float * __restrict__ out,
        uint32_t kv_head_count,
        uint32_t head_dim,
        uint32_t kv_row_stride,
        const uint32_t * __restrict__ seq_len_device_u32,
        const uint32_t * __restrict__ start_slot_device_u32,
        uint32_t capacity,
        uint32_t probs_row_stride,
        uint32_t out_row_stride) {
    const uint32_t dim = blockIdx.x;
    const uint32_t kv_head = blockIdx.y;
    if (dim >= head_dim || kv_head >= kv_head_count) {
        return;
    }

    const uint32_t seq_len = *seq_len_device_u32;
    const uint32_t start_slot = *start_slot_device_u32;
    const uint32_t q_head_base = kv_head * 8;
    const float * row_probs[8];
#pragma unroll
    for (int i = 0; i < 8; ++i) {
        row_probs[i] = probs + (q_head_base + static_cast<uint32_t>(i)) * probs_row_stride;
    }
    const uint16_t * value_row = value_cache + kv_head * kv_row_stride + dim * capacity;

    float acc[8] = {0.0f, 0.0f, 0.0f, 0.0f, 0.0f, 0.0f, 0.0f, 0.0f};
    for (uint32_t token = threadIdx.x; token < seq_len; token += blockDim.x) {
        const uint32_t slot = (start_slot + token) % capacity;
        const float value = makepad_ggml_cuda_bf16_to_f32(value_row[slot]);
#pragma unroll
        for (int i = 0; i < 8; ++i) {
            acc[i] += row_probs[i][token] * value;
        }
    }

#pragma unroll
    for (int i = 0; i < 8; ++i) {
        acc[i] = makepad_ggml_cuda_block_reduce_sum(acc[i]);
        __syncthreads();
    }
    if (threadIdx.x == 0) {
#pragma unroll
        for (int i = 0; i < 8; ++i) {
            out[(q_head_base + static_cast<uint32_t>(i)) * out_row_stride + dim] =
                makepad_ggml_cuda_bf16_round(acc[i]);
        }
    }
}

static __global__ void makepad_ggml_cuda_attention_seq_softmax_weighted_sum_f32_kernel(
        const float * __restrict__ q,
        const uint16_t * __restrict__ key_cache,
        const uint16_t * __restrict__ value_cache,
        float * __restrict__ out,
        uint32_t q_head_count,
        uint32_t q_heads_per_kv,
        uint32_t head_dim,
        uint32_t kv_row_stride,
        uint32_t seq_len,
        uint32_t start_slot,
        uint32_t capacity,
        uint32_t out_row_stride) {
    const uint32_t q_head = blockIdx.x;
    if (q_head >= q_head_count) {
        return;
    }

    extern __shared__ float shared_logits[];
    const uint32_t kv_head = q_head / q_heads_per_kv;
    const float * q_row = q + q_head * head_dim;
    const uint16_t * key_row = key_cache + kv_head * kv_row_stride;
    const uint16_t * value_row = value_cache + kv_head * kv_row_stride;

    for (uint32_t token = 0; token < seq_len; ++token) {
        const uint32_t slot = (start_slot + token) % capacity;
        const uint16_t * k_row = key_row + slot * head_dim;
        float sum = 0.0f;
        for (uint32_t dim = threadIdx.x; dim < head_dim; dim += blockDim.x) {
            sum += q_row[dim] * makepad_ggml_cuda_bf16_to_f32(k_row[dim]);
        }
        sum = makepad_ggml_cuda_block_reduce_sum(sum);
        if (threadIdx.x == 0) {
            shared_logits[token] = makepad_ggml_cuda_bf16_round(sum);
        }
        __syncthreads();
    }

    float max_value = -CUDART_INF_F;
    for (uint32_t token = threadIdx.x; token < seq_len; token += blockDim.x) {
        const float value = shared_logits[token];
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
    for (uint32_t token = threadIdx.x; token < seq_len; token += blockDim.x) {
        sum += expf(shared_logits[token] - shared_max);
    }
    sum = makepad_ggml_cuda_block_reduce_sum(sum);
    if (threadIdx.x == 0) {
        shared_sum = sum;
    }
    __syncthreads();

    for (uint32_t token = threadIdx.x; token < seq_len; token += blockDim.x) {
        shared_logits[token] = makepad_ggml_cuda_bf16_round(expf(shared_logits[token] - shared_max) / shared_sum);
    }
    __syncthreads();

    for (uint32_t dim = threadIdx.x; dim < head_dim; dim += blockDim.x) {
        float acc = 0.0f;
        for (uint32_t token = 0; token < seq_len; ++token) {
            const float prob = shared_logits[token];
            const uint32_t slot = (start_slot + token) % capacity;
            const float value = makepad_ggml_cuda_bf16_to_f32(value_row[dim * capacity + slot]);
            acc = makepad_ggml_cuda_bf16_round(acc + makepad_ggml_cuda_bf16_round(prob * value));
        }
        out[q_head * out_row_stride + dim] = acc;
    }
}

static __global__ void makepad_ggml_cuda_attention_seq_softmax_weighted_sum_f32_device_u32_kernel(
        const float * __restrict__ q,
        const uint16_t * __restrict__ key_cache,
        const uint16_t * __restrict__ value_cache,
        float * __restrict__ out,
        uint32_t q_head_count,
        uint32_t q_heads_per_kv,
        uint32_t head_dim,
        uint32_t kv_row_stride,
        const uint32_t * __restrict__ seq_len_device_u32,
        uint32_t capacity,
        uint32_t out_row_stride) {
    const uint32_t seq_len = *seq_len_device_u32;
    const uint32_t q_head = blockIdx.x;
    if (q_head >= q_head_count) {
        return;
    }

    extern __shared__ float shared_logits[];
    const uint32_t kv_head = q_head / q_heads_per_kv;
    const float * q_row = q + q_head * head_dim;
    const uint16_t * key_row = key_cache + kv_head * kv_row_stride;
    const uint16_t * value_row = value_cache + kv_head * kv_row_stride;
    const bool cache_q = head_dim <= blockDim.x;
    const float q_value = cache_q && threadIdx.x < head_dim ? q_row[threadIdx.x] : 0.0f;

    for (uint32_t token = 0; token < seq_len; ++token) {
        const uint16_t * k_row = key_row + token * head_dim;
        float sum = 0.0f;
        if (cache_q) {
            if (threadIdx.x < head_dim) {
                sum = q_value * makepad_ggml_cuda_bf16_to_f32(k_row[threadIdx.x]);
            }
        } else {
            for (uint32_t dim = threadIdx.x; dim < head_dim; dim += blockDim.x) {
                sum += q_row[dim] * makepad_ggml_cuda_bf16_to_f32(k_row[dim]);
            }
        }
        sum = makepad_ggml_cuda_block_reduce_sum(sum);
        if (threadIdx.x == 0) {
            shared_logits[token] = makepad_ggml_cuda_bf16_round(sum);
        }
        __syncthreads();
    }

    float max_value = -CUDART_INF_F;
    for (uint32_t token = threadIdx.x; token < seq_len; token += blockDim.x) {
        const float value = shared_logits[token];
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
    for (uint32_t token = threadIdx.x; token < seq_len; token += blockDim.x) {
        sum += expf(shared_logits[token] - shared_max);
    }
    sum = makepad_ggml_cuda_block_reduce_sum(sum);
    if (threadIdx.x == 0) {
        shared_sum = sum;
    }
    __syncthreads();

    for (uint32_t token = threadIdx.x; token < seq_len; token += blockDim.x) {
        shared_logits[token] = makepad_ggml_cuda_bf16_round(expf(shared_logits[token] - shared_max) / shared_sum);
    }
    __syncthreads();

    for (uint32_t dim = threadIdx.x; dim < head_dim; dim += blockDim.x) {
        float acc = 0.0f;
        for (uint32_t token = 0; token < seq_len; ++token) {
            const float prob = shared_logits[token];
            const float value = makepad_ggml_cuda_bf16_to_f32(value_row[dim * capacity + token]);
            acc = makepad_ggml_cuda_bf16_round(acc + makepad_ggml_cuda_bf16_round(prob * value));
        }
        out[q_head * out_row_stride + dim] = acc;
    }
}

static __global__ void makepad_ggml_cuda_attention_seq_softmax_weighted_sum_f32_device_u32_parallel_tokens_kernel(
        const float * __restrict__ q,
        const uint16_t * __restrict__ key_cache,
        const uint16_t * __restrict__ value_cache,
        float * __restrict__ out,
        uint32_t q_head_count,
        uint32_t q_heads_per_kv,
        uint32_t head_dim,
        uint32_t kv_row_stride,
        const uint32_t * __restrict__ seq_len_device_u32,
        uint32_t capacity,
        uint32_t out_row_stride) {
    const uint32_t seq_len = *seq_len_device_u32;
    const uint32_t q_head = blockIdx.x;
    if (q_head >= q_head_count) {
        return;
    }

    extern __shared__ float shared[];
    float * shared_logits = shared;
    float * shared_q = shared + capacity;

    const uint32_t kv_head = q_head / q_heads_per_kv;
    const float * q_row = q + q_head * head_dim;
    const uint16_t * key_row = key_cache + kv_head * kv_row_stride;
    const uint16_t * value_row = value_cache + kv_head * kv_row_stride;

    const uint32_t tid = threadIdx.x;
    const uint32_t lane = tid & 31u;
    const uint32_t group = tid >> 6;
    const uint32_t lane_in_group = tid & 63u;
    const uint32_t warp_in_group = (tid >> 5) & 1u;
    constexpr uint32_t group_size = 64;
    constexpr uint32_t groups_per_block = 256 / group_size;
    __shared__ float shared_group_partials[groups_per_block][2];

    for (uint32_t dim = tid; dim < head_dim; dim += blockDim.x) {
        shared_q[dim] = q_row[dim];
    }
    __syncthreads();

    for (uint32_t token_base = 0; token_base < seq_len; token_base += groups_per_block) {
        const uint32_t token = token_base + group;
        if (token < seq_len) {
            const uint16_t * k_row = key_row + token * head_dim;
            float sum = 0.0f;
            for (uint32_t dim = lane_in_group; dim < head_dim; dim += group_size) {
                sum += shared_q[dim] * makepad_ggml_cuda_bf16_to_f32(k_row[dim]);
            }
            sum = makepad_ggml_cuda_warp_reduce_sum(sum);
            if (lane == 0) {
                shared_group_partials[group][warp_in_group] = sum;
            }
        }
        __syncthreads();
        if (token < seq_len && warp_in_group == 0 && lane == 0) {
            shared_logits[token] = makepad_ggml_cuda_bf16_round(
                shared_group_partials[group][0] + shared_group_partials[group][1]);
        }
        __syncthreads();
    }

    float max_value = -CUDART_INF_F;
    for (uint32_t token = tid; token < seq_len; token += blockDim.x) {
        const float value = shared_logits[token];
        max_value = value > max_value ? value : max_value;
    }
    max_value = makepad_ggml_cuda_block_reduce_max(max_value);

    __shared__ float shared_max;
    __shared__ float shared_sum;
    if (tid == 0) {
        shared_max = max_value;
    }
    __syncthreads();

    float sum = 0.0f;
    for (uint32_t token = tid; token < seq_len; token += blockDim.x) {
        sum += expf(shared_logits[token] - shared_max);
    }
    sum = makepad_ggml_cuda_block_reduce_sum(sum);
    if (tid == 0) {
        shared_sum = sum;
    }
    __syncthreads();

    for (uint32_t token = tid; token < seq_len; token += blockDim.x) {
        shared_logits[token] = makepad_ggml_cuda_bf16_round(expf(shared_logits[token] - shared_max) / shared_sum);
    }
    __syncthreads();

    for (uint32_t dim = tid; dim < head_dim; dim += blockDim.x) {
        float acc = 0.0f;
        for (uint32_t token = 0; token < seq_len; ++token) {
            const float prob = shared_logits[token];
            const float value = makepad_ggml_cuda_bf16_to_f32(value_row[dim * capacity + token]);
            acc = makepad_ggml_cuda_bf16_round(acc + makepad_ggml_cuda_bf16_round(prob * value));
        }
        out[q_head * out_row_stride + dim] = acc;
    }
}

static __global__ void makepad_ggml_cuda_attention_seq_softmax_weighted_sum_rows_f32_kernel(
        const float * __restrict__ q,
        const uint16_t * __restrict__ key_cache,
        const uint16_t * __restrict__ value_cache,
        float * __restrict__ out,
        uint32_t query_count,
        uint32_t q_head_count,
        uint32_t q_heads_per_kv,
        uint32_t head_dim,
        uint32_t kv_row_stride,
        uint32_t q_row_stride,
        uint32_t out_row_stride,
        uint32_t base_seq_len,
        uint32_t capacity) {
    const uint32_t q_head = blockIdx.x;
    const uint32_t query_idx = blockIdx.y;
    if (q_head >= q_head_count || query_idx >= query_count) {
        return;
    }

    const uint32_t query_seq_len = base_seq_len + query_idx + 1u;
    const uint32_t seq_len = min(query_seq_len, capacity);
    const bool wraps = query_seq_len > capacity;
    const uint32_t start_slot = wraps ? (query_seq_len - seq_len) % capacity : 0u;
    extern __shared__ float shared_logits[];
    const uint32_t kv_head = q_head / q_heads_per_kv;
    const float * q_row = q + query_idx * q_row_stride + q_head * head_dim;
    const uint16_t * key_row = key_cache + kv_head * kv_row_stride;
    const uint16_t * value_row = value_cache + kv_head * kv_row_stride;
    const bool cache_q = head_dim <= blockDim.x;
    const float q_value = cache_q && threadIdx.x < head_dim ? q_row[threadIdx.x] : 0.0f;

    for (uint32_t token = 0; token < seq_len; ++token) {
        const uint32_t slot = wraps ? (start_slot + token) % capacity : token;
        const uint16_t * k_row = key_row + slot * head_dim;
        float sum = 0.0f;
        if (cache_q) {
            if (threadIdx.x < head_dim) {
                sum = q_value * makepad_ggml_cuda_bf16_to_f32(k_row[threadIdx.x]);
            }
        } else {
            for (uint32_t dim = threadIdx.x; dim < head_dim; dim += blockDim.x) {
                sum += q_row[dim] * makepad_ggml_cuda_bf16_to_f32(k_row[dim]);
            }
        }
        sum = makepad_ggml_cuda_block_reduce_sum(sum);
        if (threadIdx.x == 0) {
            shared_logits[token] = makepad_ggml_cuda_bf16_round(sum);
        }
        __syncthreads();
    }

    float max_value = -CUDART_INF_F;
    for (uint32_t token = threadIdx.x; token < seq_len; token += blockDim.x) {
        const float value = shared_logits[token];
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
    for (uint32_t token = threadIdx.x; token < seq_len; token += blockDim.x) {
        sum += expf(shared_logits[token] - shared_max);
    }
    sum = makepad_ggml_cuda_block_reduce_sum(sum);
    if (threadIdx.x == 0) {
        shared_sum = sum;
    }
    __syncthreads();

    for (uint32_t token = threadIdx.x; token < seq_len; token += blockDim.x) {
        shared_logits[token] = makepad_ggml_cuda_bf16_round(expf(shared_logits[token] - shared_max) / shared_sum);
    }
    __syncthreads();

    float * out_row = out + query_idx * out_row_stride + q_head * head_dim;
    for (uint32_t dim = threadIdx.x; dim < head_dim; dim += blockDim.x) {
        float acc = 0.0f;
        for (uint32_t token = 0; token < seq_len; ++token) {
            const float prob = shared_logits[token];
            const uint32_t slot = wraps ? (start_slot + token) % capacity : token;
            const float value = makepad_ggml_cuda_bf16_to_f32(value_row[dim * capacity + slot]);
            acc = makepad_ggml_cuda_bf16_round(acc + makepad_ggml_cuda_bf16_round(prob * value));
        }
        out_row[dim] = acc;
    }
}

static __global__ void makepad_ggml_cuda_attention_seq_softmax_weighted_sum_rows_f32_device_u32_kernel(
        const float * __restrict__ q,
        const uint16_t * __restrict__ key_cache,
        const uint16_t * __restrict__ value_cache,
        float * __restrict__ out,
        uint32_t query_count,
        uint32_t q_head_count,
        uint32_t q_heads_per_kv,
        uint32_t head_dim,
        uint32_t kv_row_stride,
        uint32_t q_row_stride,
        uint32_t out_row_stride,
        const uint32_t * __restrict__ base_seq_len_device_u32,
        uint32_t capacity) {
    const uint32_t q_head = blockIdx.x;
    const uint32_t query_idx = blockIdx.y;
    if (q_head >= q_head_count || query_idx >= query_count) {
        return;
    }

    const uint32_t base_seq_len = *base_seq_len_device_u32;
    const uint32_t query_seq_len = base_seq_len + query_idx + 1u;
    const uint32_t seq_len = min(query_seq_len, capacity);
    const bool wraps = query_seq_len > capacity;
    const uint32_t start_slot = wraps ? (query_seq_len - seq_len) % capacity : 0u;
    extern __shared__ float shared_logits[];
    const uint32_t kv_head = q_head / q_heads_per_kv;
    const float * q_row = q + query_idx * q_row_stride + q_head * head_dim;
    const uint16_t * key_row = key_cache + kv_head * kv_row_stride;
    const uint16_t * value_row = value_cache + kv_head * kv_row_stride;
    const bool cache_q = head_dim <= blockDim.x;
    const float q_value = cache_q && threadIdx.x < head_dim ? q_row[threadIdx.x] : 0.0f;

    for (uint32_t token = 0; token < seq_len; ++token) {
        const uint32_t slot = wraps ? (start_slot + token) % capacity : token;
        const uint16_t * k_row = key_row + slot * head_dim;
        float sum = 0.0f;
        if (cache_q) {
            if (threadIdx.x < head_dim) {
                sum = q_value * makepad_ggml_cuda_bf16_to_f32(k_row[threadIdx.x]);
            }
        } else {
            for (uint32_t dim = threadIdx.x; dim < head_dim; dim += blockDim.x) {
                sum += q_row[dim] * makepad_ggml_cuda_bf16_to_f32(k_row[dim]);
            }
        }
        sum = makepad_ggml_cuda_block_reduce_sum(sum);
        if (threadIdx.x == 0) {
            shared_logits[token] = makepad_ggml_cuda_bf16_round(sum);
        }
        __syncthreads();
    }

    float max_value = -CUDART_INF_F;
    for (uint32_t token = threadIdx.x; token < seq_len; token += blockDim.x) {
        const float value = shared_logits[token];
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
    for (uint32_t token = threadIdx.x; token < seq_len; token += blockDim.x) {
        sum += expf(shared_logits[token] - shared_max);
    }
    sum = makepad_ggml_cuda_block_reduce_sum(sum);
    if (threadIdx.x == 0) {
        shared_sum = sum;
    }
    __syncthreads();

    for (uint32_t token = threadIdx.x; token < seq_len; token += blockDim.x) {
        shared_logits[token] = makepad_ggml_cuda_bf16_round(expf(shared_logits[token] - shared_max) / shared_sum);
    }
    __syncthreads();

    float * out_row = out + query_idx * out_row_stride + q_head * head_dim;
    for (uint32_t dim = threadIdx.x; dim < head_dim; dim += blockDim.x) {
        float acc = 0.0f;
        for (uint32_t token = 0; token < seq_len; ++token) {
            const float prob = shared_logits[token];
            const uint32_t slot = wraps ? (start_slot + token) % capacity : token;
            const float value = makepad_ggml_cuda_bf16_to_f32(value_row[dim * capacity + slot]);
            acc = makepad_ggml_cuda_bf16_round(acc + makepad_ggml_cuda_bf16_round(prob * value));
        }
        out_row[dim] = acc;
    }
}

static __global__ void makepad_ggml_cuda_flash_attn_f32_packed_kernel(
        const float * __restrict__ q,
        const float * __restrict__ k,
        const float * __restrict__ v,
        float * __restrict__ out,
        uint32_t seq_len,
        uint32_t num_heads,
        uint32_t head_dim,
        float scale) {
    const uint32_t head = blockIdx.x;
    const uint32_t query = blockIdx.y;
    if (head >= num_heads || query >= seq_len) {
        return;
    }

    extern __shared__ float shared_logits[];
    const uint32_t row_stride = num_heads * head_dim;
    const float * q_row = q + query * row_stride + head * head_dim;
    for (uint32_t token = 0; token < seq_len; ++token) {
        const float * k_row = k + token * row_stride + head * head_dim;
        float sum = 0.0f;
        for (uint32_t dim = threadIdx.x; dim < head_dim; dim += blockDim.x) {
            sum += q_row[dim] * k_row[dim];
        }
        sum = makepad_ggml_cuda_block_reduce_sum(sum);
        if (threadIdx.x == 0) {
            shared_logits[token] = sum * scale;
        }
        __syncthreads();
    }

    float max_value = -CUDART_INF_F;
    for (uint32_t token = threadIdx.x; token < seq_len; token += blockDim.x) {
        const float value = shared_logits[token];
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
    for (uint32_t token = threadIdx.x; token < seq_len; token += blockDim.x) {
        sum += expf(shared_logits[token] - shared_max);
    }
    sum = makepad_ggml_cuda_block_reduce_sum(sum);
    if (threadIdx.x == 0) {
        shared_sum = sum;
    }
    __syncthreads();

    float * out_row = out + query * row_stride + head * head_dim;
    for (uint32_t dim = threadIdx.x; dim < head_dim; dim += blockDim.x) {
        float acc = 0.0f;
        for (uint32_t token = 0; token < seq_len; ++token) {
            const float prob = expf(shared_logits[token] - shared_max) / shared_sum;
            const float value = v[token * row_stride + head * head_dim + dim];
            acc += prob * value;
        }
        out_row[dim] = acc;
    }
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

    __shared__ float shared_values[1024];
    __shared__ uint32_t shared_indices[1024];
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

static __global__ void makepad_ggml_cuda_mask_indices_f32_kernel(
        float * __restrict__ logits,
        const uint32_t * __restrict__ disallowed_token_ids,
        uint32_t disallowed_count,
        uint32_t n) {
    const uint32_t index = blockIdx.x * blockDim.x + threadIdx.x;
    if (index >= disallowed_count) {
        return;
    }
    const uint32_t token_id = disallowed_token_ids[index];
    if (token_id < n) {
        logits[token_id] = -CUDART_INF_F;
    }
}

static __global__ void makepad_ggml_cuda_mask_indices_f32_device_u32_kernel(
        float * __restrict__ logits,
        const uint32_t * __restrict__ disallowed_token_ids,
        const uint32_t * __restrict__ disallowed_count_device_u32,
        uint32_t n) {
    const uint32_t disallowed_count = *disallowed_count_device_u32;
    const uint32_t index = blockIdx.x * blockDim.x + threadIdx.x;
    if (index >= disallowed_count) {
        return;
    }
    const uint32_t token_id = disallowed_token_ids[index];
    if (token_id < n) {
        logits[token_id] = -CUDART_INF_F;
    }
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

extern "C" cudaError_t makepad_ggml_cuda_scale_f32_inplace_device_f32_index(
        float * values,
        const float * scales,
        uint32_t scale_index,
        uint32_t n,
        cudaStream_t stream) {
    if (n == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid((n + block.x - 1) / block.x, 1, 1);
    makepad_ggml_cuda_scale_f32_device_f32_index_kernel<<<grid, block, 0, stream>>>(
        values,
        scales,
        scale_index,
        n);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_f32_to_bf16(
        const float * input,
        uint16_t * output,
        uint32_t n,
        cudaStream_t stream) {
    if (n == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid((n + block.x - 1) / block.x, 1, 1);
    makepad_ggml_cuda_f32_to_bf16_kernel<<<grid, block, 0, stream>>>(input, output, n);
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

extern "C" cudaError_t makepad_ggml_cuda_weighted_sum_rows_f32(
        const float * batched_inputs,
        const float * weights,
        float * output,
        uint32_t row_count,
        uint32_t input_count,
        cudaStream_t stream) {
    if (row_count == 0 || input_count == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid((row_count + block.x - 1) / block.x, 1, 1);
    makepad_ggml_cuda_weighted_sum_rows_f32_kernel<<<grid, block, 0, stream>>>(
        batched_inputs,
        weights,
        output,
        row_count,
        input_count);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_mul_f32(
        const float * left,
        const float * right,
        float * out,
        uint32_t n,
        cudaStream_t stream) {
    if (n == 0) {
        return cudaErrorInvalidValue;
    }
    const uint32_t block = 256;
    const uint32_t grid = (n + block - 1) / block;
    makepad_ggml_cuda_mul_f32_kernel<<<grid, block, 0, stream>>>(left, right, out, n);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_gelu_f32(
        const float * input,
        float * out,
        uint32_t n,
        cudaStream_t stream) {
    if (n == 0) {
        return cudaErrorInvalidValue;
    }
    const uint32_t block = 256;
    const uint32_t grid = (n + block - 1) / block;
    makepad_ggml_cuda_gelu_f32_kernel<<<grid, block, 0, stream>>>(input, out, n);
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
    const uint32_t block = n < 1024 ? 256 : 1024;
    makepad_ggml_cuda_rms_norm_row_weighted_f32_kernel<<<1, block, 0, stream>>>(input, weights_bf16, output, n, eps);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_rms_norm_row_weighted_f32_f32weights(
        const float * input,
        const float * weights_f32,
        float * output,
        uint32_t n,
        float eps,
        cudaStream_t stream) {
    if (n == 0) {
        return cudaErrorInvalidValue;
    }
    const uint32_t block = n < 1024 ? 256 : 1024;
    makepad_ggml_cuda_rms_norm_row_weighted_f32_f32weights_kernel<<<1, block, 0, stream>>>(
        input, weights_f32, output, n, eps);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_rms_norm_row_weighted_f32_f32weights_precise(
        const float * input,
        const float * weights_f32,
        float * output,
        uint32_t n,
        float eps,
        cudaStream_t stream) {
    if (n == 0) {
        return cudaErrorInvalidValue;
    }
    const uint32_t block = n < 1024 ? 256 : 1024;
    makepad_ggml_cuda_rms_norm_row_weighted_f32_f32weights_precise_kernel<<<1, block, 0, stream>>>(
        input, weights_f32, output, n, eps);
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
    const uint32_t block = n < 1024 ? 256 : 1024;
    makepad_ggml_cuda_rms_norm_rows_weighted_f32_kernel<<<row_count, block, 0, stream>>>(
        input, weights_bf16, output, row_count, row_stride, n, eps);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_rms_norm_rows_weighted_f32_f32weights(
        const float * input,
        const float * weights_f32,
        float * output,
        uint32_t row_count,
        uint32_t row_stride,
        uint32_t n,
        float eps,
        cudaStream_t stream) {
    if (row_count == 0 || n == 0 || row_stride < n) {
        return cudaErrorInvalidValue;
    }
    const uint32_t block = n < 1024 ? 256 : 1024;
    makepad_ggml_cuda_rms_norm_rows_weighted_f32_f32weights_kernel<<<row_count, block, 0, stream>>>(
        input, weights_f32, output, row_count, row_stride, n, eps);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_rms_norm_rows_weighted_f32_f32weights_precise(
        const float * input,
        const float * weights_f32,
        float * output,
        uint32_t row_count,
        uint32_t row_stride,
        uint32_t n,
        float eps,
        cudaStream_t stream) {
    if (row_count == 0 || n == 0 || row_stride < n) {
        return cudaErrorInvalidValue;
    }
    const uint32_t block = n < 1024 ? 256 : 1024;
    makepad_ggml_cuda_rms_norm_rows_weighted_f32_f32weights_precise_kernel<<<row_count, block, 0, stream>>>(
        input, weights_f32, output, row_count, row_stride, n, eps);
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

extern "C" cudaError_t makepad_ggml_cuda_rms_norm_rows_no_scale_f32_precise(
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
    makepad_ggml_cuda_rms_norm_rows_no_scale_f32_precise_kernel<<<row_count, 256, 0, stream>>>(
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
        uint16_t * key_cache,
        uint16_t * value_cache,
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
        uint16_t * key_cache,
        uint16_t * value_cache,
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
        uint16_t * key_cache,
        uint16_t * value_cache,
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

extern "C" cudaError_t makepad_ggml_cuda_qkv_norm_rope_cache_rows_f32(
        const float * qkv,
        const uint16_t * q_weights_bf16,
        const uint16_t * k_weights_bf16,
        float * q_out,
        uint16_t * key_cache,
        uint16_t * value_cache,
        uint32_t q_head_count,
        uint32_t k_head_count,
        uint32_t head_dim,
        uint32_t qkv_row_stride,
        uint32_t q_out_row_stride,
        uint32_t q_offset,
        uint32_t k_offset,
        uint32_t v_offset,
        uint32_t rotary_dim,
        float base,
        uint32_t start_position,
        float eps,
        uint32_t max_tokens,
        uint32_t start_slot,
        uint32_t row_count,
        cudaStream_t stream) {
    if (row_count == 0 || max_tokens == 0) {
        return cudaErrorInvalidValue;
    }
    const uint32_t total_rows = row_count * (q_head_count + 2u * k_head_count);
    makepad_ggml_cuda_qkv_norm_rope_cache_rows_f32_kernel<<<total_rows, 256, 0, stream>>>(
        qkv,
        q_weights_bf16,
        k_weights_bf16,
        q_out,
        key_cache,
        value_cache,
        q_head_count,
        k_head_count,
        head_dim,
        qkv_row_stride,
        q_out_row_stride,
        q_offset,
        k_offset,
        v_offset,
        rotary_dim,
        base,
        start_position,
        eps,
        max_tokens,
        start_slot,
        row_count);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_qkv_norm_rope_cache_f32_device_u32(
        const float * qkv,
        const uint16_t * q_weights_bf16,
        const uint16_t * k_weights_bf16,
        float * q_out,
        uint16_t * key_cache,
        uint16_t * value_cache,
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

extern "C" cudaError_t makepad_ggml_cuda_qkv_norm_rope_cache_rows_f32_device_u32(
        const float * qkv,
        const uint16_t * q_weights_bf16,
        const uint16_t * k_weights_bf16,
        float * q_out,
        uint16_t * key_cache,
        uint16_t * value_cache,
        uint32_t q_head_count,
        uint32_t k_head_count,
        uint32_t head_dim,
        uint32_t qkv_row_stride,
        uint32_t q_out_row_stride,
        uint32_t q_offset,
        uint32_t k_offset,
        uint32_t v_offset,
        uint32_t rotary_dim,
        float base,
        const uint32_t * start_position_device_u32,
        float eps,
        uint32_t max_tokens,
        const uint32_t * start_slot_device_u32,
        uint32_t row_count,
        cudaStream_t stream) {
    if (row_count == 0 || max_tokens == 0) {
        return cudaErrorInvalidValue;
    }
    const uint32_t total_rows = row_count * (q_head_count + 2u * k_head_count);
    makepad_ggml_cuda_qkv_norm_rope_cache_rows_f32_device_u32_kernel<<<total_rows, 256, 0, stream>>>(
        qkv,
        q_weights_bf16,
        k_weights_bf16,
        q_out,
        key_cache,
        value_cache,
        q_head_count,
        k_head_count,
        head_dim,
        qkv_row_stride,
        q_out_row_stride,
        q_offset,
        k_offset,
        v_offset,
        rotary_dim,
        base,
        start_position_device_u32,
        eps,
        max_tokens,
        start_slot_device_u32,
        row_count);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_attention_logits_seq_f32(
        const float * q,
        const uint16_t * key_cache,
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
        const uint16_t * key_cache,
        float * logits,
        uint32_t q_head_count,
        uint32_t q_heads_per_kv,
        uint32_t head_dim,
        uint32_t kv_row_stride,
        const uint32_t * seq_len_device_u32,
        const uint32_t * start_slot_device_u32,
        uint32_t capacity,
        uint32_t logits_row_stride,
        cudaStream_t stream) {
    if (q_head_count == 0 || q_heads_per_kv == 0 || head_dim == 0 || capacity == 0 || logits_row_stride < capacity) {
        return cudaErrorInvalidValue;
    }
    constexpr uint32_t tokens_per_block = 4;
    const dim3 grid(q_head_count, (capacity + tokens_per_block - 1) / tokens_per_block, 1);
    makepad_ggml_cuda_attention_logits_seq_f32_device_u32_vec2_kernel<tokens_per_block><<<grid, 128, 0, stream>>>(
        q, key_cache, logits, q_head_count, q_heads_per_kv, head_dim, kv_row_stride, seq_len_device_u32, start_slot_device_u32, capacity, logits_row_stride);
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

extern "C" cudaError_t makepad_ggml_cuda_softmax_rows_causal_f32(
        float * logits,
        uint32_t query_count,
        uint32_t row_count,
        uint32_t row_stride,
        uint32_t base_seq_len,
        uint32_t max_seq_len,
        cudaStream_t stream) {
    if (query_count == 0 || row_count == 0 || row_stride < max_seq_len || max_seq_len == 0) {
        return cudaErrorInvalidValue;
    }
    makepad_ggml_cuda_softmax_rows_causal_f32_kernel<<<row_count, 256, 0, stream>>>(
        logits, query_count, row_count, row_stride, base_seq_len, max_seq_len);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_softmax_rows_causal_f32_device_u32(
        float * logits,
        uint32_t query_count,
        uint32_t row_count,
        uint32_t row_stride,
        const uint32_t * base_seq_len_device_u32,
        uint32_t max_seq_len,
        cudaStream_t stream) {
    if (query_count == 0 || row_count == 0 || row_stride < max_seq_len || max_seq_len == 0) {
        return cudaErrorInvalidValue;
    }
    makepad_ggml_cuda_softmax_rows_causal_f32_device_u32_kernel<<<row_count, 256, 0, stream>>>(
        logits, query_count, row_count, row_stride, base_seq_len_device_u32, max_seq_len);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_softmax_rows_causal_bf16(
        const float * logits,
        uint16_t * probs,
        uint32_t query_count,
        uint32_t row_count,
        uint32_t row_stride,
        uint32_t base_seq_len,
        uint32_t max_seq_len,
        cudaStream_t stream) {
    if (query_count == 0 || row_count == 0 || row_stride < max_seq_len || max_seq_len == 0) {
        return cudaErrorInvalidValue;
    }
    makepad_ggml_cuda_softmax_rows_causal_bf16_kernel<<<row_count, 256, 0, stream>>>(
        logits, probs, query_count, row_count, row_stride, base_seq_len, max_seq_len);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_softmax_rows_causal_bf16_device_u32(
        const float * logits,
        uint16_t * probs,
        uint32_t query_count,
        uint32_t row_count,
        uint32_t row_stride,
        const uint32_t * base_seq_len_device_u32,
        uint32_t max_seq_len,
        cudaStream_t stream) {
    if (query_count == 0 || row_count == 0 || row_stride < max_seq_len || max_seq_len == 0) {
        return cudaErrorInvalidValue;
    }
    makepad_ggml_cuda_softmax_rows_causal_bf16_device_u32_kernel<<<row_count, 256, 0, stream>>>(
        logits, probs, query_count, row_count, row_stride, base_seq_len_device_u32, max_seq_len);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_softmax_rows_causal_vision_bf16(
        const float * logits,
        uint16_t * probs,
        uint32_t query_count,
        uint32_t row_count,
        uint32_t row_stride,
        uint32_t base_seq_len,
        uint32_t max_seq_len,
        uint32_t chunk_start_position,
        uint32_t vision_start_position,
        uint32_t vision_end_position,
        cudaStream_t stream) {
    if (query_count == 0
            || row_count == 0
            || row_stride < max_seq_len
            || max_seq_len == 0
            || vision_start_position > vision_end_position) {
        return cudaErrorInvalidValue;
    }
    makepad_ggml_cuda_softmax_rows_causal_vision_bf16_kernel<<<row_count, 256, 0, stream>>>(
        logits,
        probs,
        query_count,
        row_count,
        row_stride,
        base_seq_len,
        max_seq_len,
        chunk_start_position,
        vision_start_position,
        vision_end_position);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_attention_weighted_sum_f32(
        const float * probs,
        const uint16_t * value_cache,
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

extern "C" cudaError_t makepad_ggml_cuda_attention_softmax_weighted_sum_f32(
        const float * logits,
        const uint16_t * value_cache,
        float * out,
        uint32_t q_head_count,
        uint32_t q_heads_per_kv,
        uint32_t head_dim,
        uint32_t kv_row_stride,
        uint32_t seq_len,
        uint32_t start_slot,
        uint32_t capacity,
        uint32_t logits_row_stride,
        uint32_t out_row_stride,
        cudaStream_t stream) {
    if (q_head_count == 0 || q_heads_per_kv == 0 || head_dim == 0 || seq_len == 0 || capacity == 0 || start_slot >= capacity || logits_row_stride < seq_len || out_row_stride < head_dim) {
        return cudaErrorInvalidValue;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid((head_dim + block.x - 1) / block.x, q_head_count, 1);
    const size_t shared_bytes = static_cast<size_t>(seq_len) * sizeof(float);
    makepad_ggml_cuda_attention_softmax_weighted_sum_f32_kernel<<<grid, block, shared_bytes, stream>>>(
        logits, value_cache, out, q_head_count, q_heads_per_kv, head_dim, kv_row_stride, seq_len, start_slot, capacity, logits_row_stride, out_row_stride);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_attention_weighted_sum_f32_device_u32(
        const float * probs,
        const uint16_t * value_cache,
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

extern "C" cudaError_t makepad_ggml_cuda_attention_softmax_weighted_sum_f32_device_u32(
        const float * logits,
        const uint16_t * value_cache,
        float * out,
        uint32_t q_head_count,
        uint32_t q_heads_per_kv,
        uint32_t head_dim,
        uint32_t kv_row_stride,
        const uint32_t * seq_len_device_u32,
        const uint32_t * start_slot_device_u32,
        uint32_t capacity,
        uint32_t logits_row_stride,
        uint32_t out_row_stride,
        cudaStream_t stream) {
    if (q_head_count == 0 || q_heads_per_kv == 0 || head_dim == 0 || capacity == 0 || logits_row_stride < capacity || out_row_stride < head_dim) {
        return cudaErrorInvalidValue;
    }
    const dim3 block(256, 1, 1);
    if (q_heads_per_kv == 2 && (q_head_count % 2) == 0) {
        makepad_ggml_cuda_softmax_rows_f32_device_u32_kernel<<<q_head_count, 256, 0, stream>>>(
            logits, const_cast<float *>(logits), q_head_count, logits_row_stride, seq_len_device_u32);
        cudaError_t status = cudaGetLastError();
        if (status != cudaSuccess) {
            return status;
        }
        const dim3 pv_block(128, 1, 1);
        const dim3 pv_grid(head_dim, q_head_count / 2, 1);
        makepad_ggml_cuda_attention_weighted_sum_transposed_pair_f32_device_u32_kernel<<<pv_grid, pv_block, 0, stream>>>(
            logits, value_cache, out, q_head_count / 2, head_dim, kv_row_stride, seq_len_device_u32, start_slot_device_u32, capacity, logits_row_stride, out_row_stride);
    } else if (q_heads_per_kv == 8 && (q_head_count % 8) == 0) {
        makepad_ggml_cuda_softmax_rows_f32_device_u32_kernel<<<q_head_count, 256, 0, stream>>>(
            logits, const_cast<float *>(logits), q_head_count, logits_row_stride, seq_len_device_u32);
        cudaError_t status = cudaGetLastError();
        if (status != cudaSuccess) {
            return status;
        }
        const dim3 pv_block(128, 1, 1);
        const dim3 pv_grid(head_dim, q_head_count / 8, 1);
        makepad_ggml_cuda_attention_weighted_sum_transposed_group8_f32_device_u32_kernel<<<pv_grid, pv_block, 0, stream>>>(
            logits, value_cache, out, q_head_count / 8, head_dim, kv_row_stride, seq_len_device_u32, start_slot_device_u32, capacity, logits_row_stride, out_row_stride);
    } else {
        const dim3 grid((head_dim + block.x - 1) / block.x, q_head_count, 1);
        const size_t shared_bytes = static_cast<size_t>(capacity) * sizeof(float);
        makepad_ggml_cuda_attention_softmax_weighted_sum_f32_device_u32_kernel<<<grid, block, shared_bytes, stream>>>(
            logits, value_cache, out, q_head_count, q_heads_per_kv, head_dim, kv_row_stride, seq_len_device_u32, start_slot_device_u32, capacity, logits_row_stride, out_row_stride);
    }
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_attention_seq_softmax_weighted_sum_f32(
        const float * q,
        const uint16_t * key_cache,
        const uint16_t * value_cache,
        float * out,
        uint32_t q_head_count,
        uint32_t q_heads_per_kv,
        uint32_t head_dim,
        uint32_t kv_row_stride,
        uint32_t seq_len,
        uint32_t start_slot,
        uint32_t capacity,
        uint32_t out_row_stride,
        cudaStream_t stream) {
    if (q_head_count == 0 || q_heads_per_kv == 0 || head_dim == 0 || seq_len == 0 || capacity == 0 || start_slot >= capacity || out_row_stride < head_dim) {
        return cudaErrorInvalidValue;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid(q_head_count, 1, 1);
    const size_t shared_bytes = static_cast<size_t>(seq_len) * sizeof(float);
    makepad_ggml_cuda_attention_seq_softmax_weighted_sum_f32_kernel<<<grid, block, shared_bytes, stream>>>(
        q, key_cache, value_cache, out, q_head_count, q_heads_per_kv, head_dim, kv_row_stride, seq_len, start_slot, capacity, out_row_stride);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_attention_seq_softmax_weighted_sum_rows_f32(
        const float * q,
        const uint16_t * key_cache,
        const uint16_t * value_cache,
        float * out,
        uint32_t query_count,
        uint32_t q_head_count,
        uint32_t q_heads_per_kv,
        uint32_t head_dim,
        uint32_t kv_row_stride,
        uint32_t q_row_stride,
        uint32_t out_row_stride,
        uint32_t base_seq_len,
        uint32_t capacity,
        cudaStream_t stream) {
    if (query_count == 0 || q_head_count == 0 || head_dim == 0 || capacity == 0) {
        return cudaErrorInvalidValue;
    }
    const dim3 grid(q_head_count, query_count, 1);
    const dim3 block(256, 1, 1);
    const size_t shared_bytes = static_cast<size_t>(min(base_seq_len + query_count, capacity)) * sizeof(float);
    makepad_ggml_cuda_attention_seq_softmax_weighted_sum_rows_f32_kernel<<<grid, block, shared_bytes, stream>>>(
        q,
        key_cache,
        value_cache,
        out,
        query_count,
        q_head_count,
        q_heads_per_kv,
        head_dim,
        kv_row_stride,
        q_row_stride,
        out_row_stride,
        base_seq_len,
        capacity);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_attention_seq_softmax_weighted_sum_f32_device_u32(
        const float * q,
        const uint16_t * key_cache,
        const uint16_t * value_cache,
        float * out,
        uint32_t q_head_count,
        uint32_t q_heads_per_kv,
        uint32_t head_dim,
        uint32_t kv_row_stride,
        const uint32_t * seq_len_device_u32,
        uint32_t capacity,
        uint32_t out_row_stride,
        cudaStream_t stream) {
    if (q_head_count == 0 || q_heads_per_kv == 0 || head_dim == 0 || capacity == 0 || out_row_stride < head_dim) {
        return cudaErrorInvalidValue;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid(q_head_count, 1, 1);
    const size_t shared_bytes = static_cast<size_t>(capacity) * sizeof(float);
    makepad_ggml_cuda_attention_seq_softmax_weighted_sum_f32_device_u32_kernel<<<grid, block, shared_bytes, stream>>>(
        q, key_cache, value_cache, out, q_head_count, q_heads_per_kv, head_dim, kv_row_stride, seq_len_device_u32, capacity, out_row_stride);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_attention_seq_softmax_weighted_sum_rows_f32_device_u32(
        const float * q,
        const uint16_t * key_cache,
        const uint16_t * value_cache,
        float * out,
        uint32_t query_count,
        uint32_t q_head_count,
        uint32_t q_heads_per_kv,
        uint32_t head_dim,
        uint32_t kv_row_stride,
        uint32_t q_row_stride,
        uint32_t out_row_stride,
        const uint32_t * base_seq_len_device_u32,
        uint32_t capacity,
        cudaStream_t stream) {
    if (query_count == 0 || q_head_count == 0 || head_dim == 0 || capacity == 0) {
        return cudaErrorInvalidValue;
    }
    const dim3 grid(q_head_count, query_count, 1);
    const dim3 block(256, 1, 1);
    const size_t shared_bytes = static_cast<size_t>(capacity) * sizeof(float);
    makepad_ggml_cuda_attention_seq_softmax_weighted_sum_rows_f32_device_u32_kernel<<<grid, block, shared_bytes, stream>>>(
        q,
        key_cache,
        value_cache,
        out,
        query_count,
        q_head_count,
        q_heads_per_kv,
        head_dim,
        kv_row_stride,
        q_row_stride,
        out_row_stride,
        base_seq_len_device_u32,
        capacity);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_flash_attn_f32_packed(
        const float * q,
        const float * k,
        const float * v,
        float * out,
        uint32_t seq_len,
        uint32_t num_heads,
        uint32_t head_dim,
        float scale,
        cudaStream_t stream) {
    if (seq_len == 0 || num_heads == 0 || head_dim == 0) {
        return cudaErrorInvalidValue;
    }
    const dim3 grid(num_heads, seq_len, 1);
    const dim3 block(256, 1, 1);
    const size_t shared_bytes = static_cast<size_t>(seq_len) * sizeof(float);
    makepad_ggml_cuda_flash_attn_f32_packed_kernel<<<grid, block, shared_bytes, stream>>>(
        q, k, v, out, seq_len, num_heads, head_dim, scale);
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
    makepad_ggml_cuda_argmax_f32_kernel<<<1, 1024, 0, stream>>>(logits, out_index, n);
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
    if (disallowed_count == 0) {
        makepad_ggml_cuda_argmax_f32_kernel<<<1, 256, 0, stream>>>(logits, out_index, n);
        return cudaGetLastError();
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
