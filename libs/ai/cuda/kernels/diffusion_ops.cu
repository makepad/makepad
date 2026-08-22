// Precise f32 kernels backing the diffusion (Flux) lazy path on CUDA.
//
// The pre-existing elementwise kernels in ops.cu truncate their results to
// bf16 (makepad_cuda_bf16_round) to mirror the LLM runtime's storage
// format. The diffusion pipeline mirrors Metal semantics, which are full-f32,
// so this translation unit provides exact f32 variants plus the ops the
// Metal compat layer needs that had no CUDA kernel at all
// (layer_norm_mul_add, row-broadcast mul, precise row softmax, f32->f16).

#include <cuda_fp16.h>
#include <cuda_bf16.h>
#include <cuda_runtime.h>
#include <math_constants.h>
#include <mma.h>
#include <stdint.h>

__device__ __forceinline__ float makepad_cuda_diff_warp_reduce_sum(float value) {
    for (int offset = warpSize / 2; offset > 0; offset >>= 1) {
        value += __shfl_down_sync(0xffffffffu, value, offset);
    }
    return value;
}

__device__ __forceinline__ float makepad_cuda_diff_warp_reduce_max(float value) {
    for (int offset = warpSize / 2; offset > 0; offset >>= 1) {
        const float other = __shfl_down_sync(0xffffffffu, value, offset);
        value = value > other ? value : other;
    }
    return value;
}

__device__ __forceinline__ float makepad_cuda_diff_block_reduce_sum(float value) {
    __shared__ float shared[32];
    const int lane = threadIdx.x & 31;
    const int warp = threadIdx.x >> 5;
    value = makepad_cuda_diff_warp_reduce_sum(value);
    if (lane == 0) {
        shared[warp] = value;
    }
    __syncthreads();
    value = threadIdx.x < (blockDim.x + 31) / 32 ? shared[lane] : 0.0f;
    if (warp == 0) {
        value = makepad_cuda_diff_warp_reduce_sum(value);
    }
    return value;
}

__device__ __forceinline__ float makepad_cuda_diff_block_reduce_max(float value) {
    __shared__ float shared[32];
    const int lane = threadIdx.x & 31;
    const int warp = threadIdx.x >> 5;
    value = makepad_cuda_diff_warp_reduce_max(value);
    if (lane == 0) {
        shared[warp] = value;
    }
    __syncthreads();
    value = threadIdx.x < (blockDim.x + 31) / 32 ? shared[lane] : -CUDART_INF_F;
    if (warp == 0) {
        value = makepad_cuda_diff_warp_reduce_max(value);
    }
    return value;
}

static __global__ void makepad_cuda_bf16_round_f32_kernel(
        const float * __restrict__ input,
        float * __restrict__ output,
        uint32_t n) {
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx < n) {
        output[idx] = __bfloat162float(__float2bfloat16_rn(input[idx]));
    }
}

static __global__ void makepad_cuda_add_bf16_f32_kernel(
        const float * __restrict__ left,
        const float * __restrict__ right,
        float * __restrict__ output,
        uint32_t n) {
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx < n) {
        output[idx] = __bfloat162float(__float2bfloat16_rn(left[idx] + right[idx]));
    }
}

extern "C" cudaError_t makepad_cuda_bf16_round_f32(
        const float * input,
        float * output,
        uint32_t n,
        cudaStream_t stream) {
    if (n == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid((n + block.x - 1) / block.x, 1, 1);
    makepad_cuda_bf16_round_f32_kernel<<<grid, block, 0, stream>>>(input, output, n);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_cuda_add_bf16_f32(
        const float * left,
        const float * right,
        float * output,
        uint32_t n,
        cudaStream_t stream) {
    if (n == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid((n + block.x - 1) / block.x, 1, 1);
    makepad_cuda_add_bf16_f32_kernel<<<grid, block, 0, stream>>>(
        left, right, output, n);
    return cudaGetLastError();
}

// out[r][c] = (x[r][c] - mean_r) * rsqrt(var_r + eps) * (gamma[c] + gamma_add)
//           + beta[c]
// Mean/variance are computed over the row (biased variance, divide by cols),
// eps sits inside the sqrt — matching kernel_norm_mul_add_f32 in
// ggml-metal.metal and the scalar reference in libs/diffusion. gamma_add
// lets modulation scales stay device-resident (the host path pre-adds 1).
static __global__ void makepad_cuda_layer_norm_mul_add_f32_kernel(
        const float * __restrict__ input,
        const float * __restrict__ gamma,
        const float * __restrict__ beta,
        float * __restrict__ output,
        uint32_t row_count,
        uint32_t cols,
        float eps,
        float gamma_add) {
    const uint32_t row = blockIdx.x;
    if (row >= row_count) {
        return;
    }
    const float * row_in = input + static_cast<size_t>(row) * cols;
    float * row_out = output + static_cast<size_t>(row) * cols;

    float sum = 0.0f;
    for (uint32_t idx = threadIdx.x; idx < cols; idx += blockDim.x) {
        sum += row_in[idx];
    }
    sum = makepad_cuda_diff_block_reduce_sum(sum);
    __shared__ float shared_mean;
    __shared__ float shared_inv;
    if (threadIdx.x == 0) {
        shared_mean = sum / static_cast<float>(cols);
    }
    __syncthreads();

    const float mean = shared_mean;
    float sq_sum = 0.0f;
    for (uint32_t idx = threadIdx.x; idx < cols; idx += blockDim.x) {
        const float centered = row_in[idx] - mean;
        sq_sum += centered * centered;
    }
    sq_sum = makepad_cuda_diff_block_reduce_sum(sq_sum);
    if (threadIdx.x == 0) {
        shared_inv = rsqrtf(sq_sum / static_cast<float>(cols) + eps);
    }
    __syncthreads();

    const float inv = shared_inv;
    for (uint32_t idx = threadIdx.x; idx < cols; idx += blockDim.x) {
        row_out[idx] = (row_in[idx] - mean) * inv * (gamma[idx] + gamma_add) + beta[idx];
    }
}

// out[r][c] = residual[r][c] + gate[c] * update[r][c] in one pass (the
// two-kernel mul_rows_vec + add recipe read the update twice).
static __global__ void makepad_cuda_gated_residual_vec_f32_kernel(
        const float * __restrict__ residual,
        const float * __restrict__ update,
        const float * __restrict__ gate,
        float * __restrict__ output,
        uint32_t row_count,
        uint32_t cols) {
    const size_t total = static_cast<size_t>(row_count) * cols;
    const size_t idx = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    if (idx < total) {
        output[idx] = fmaf(gate[idx % cols], update[idx], residual[idx]);
    }
}

extern "C" cudaError_t makepad_cuda_gated_residual_vec_f32(
        const float * residual,
        const float * update,
        const float * gate,
        float * output,
        uint32_t row_count,
        uint32_t cols,
        cudaStream_t stream) {
    const size_t total = static_cast<size_t>(row_count) * cols;
    if (total == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<unsigned int>((total + block.x - 1) / block.x), 1, 1);
    makepad_cuda_gated_residual_vec_f32_kernel<<<grid, block, 0, stream>>>(
        residual, update, gate, output, row_count, cols);
    return cudaGetLastError();
}

// gated_residual_vec with the bf16-RN boundary folded onto the store: the
// flux2 DiT rounds every residual join, and the separate bf16_round pass
// costs a full extra read+write of the 30MB stream tensor. Same fmaf then
// __float2bfloat16_rn — bit-identical to gated_residual_vec + bf16_round.
static __global__ void makepad_cuda_gated_residual_vec_round_bf16_f32_kernel(
        const float * __restrict__ residual,
        const float * __restrict__ update,
        const float * __restrict__ gate,
        float * __restrict__ output,
        uint32_t row_count,
        uint32_t cols) {
    const size_t total = static_cast<size_t>(row_count) * cols;
    const size_t idx = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    if (idx < total) {
        output[idx] = __bfloat162float(__float2bfloat16_rn(
            fmaf(gate[idx % cols], update[idx], residual[idx])));
    }
}

extern "C" cudaError_t makepad_cuda_gated_residual_vec_round_bf16_f32(
        const float * residual,
        const float * update,
        const float * gate,
        float * output,
        uint32_t row_count,
        uint32_t cols,
        cudaStream_t stream) {
    const size_t total = static_cast<size_t>(row_count) * cols;
    if (total == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<unsigned int>((total + block.x - 1) / block.x), 1, 1);
    makepad_cuda_gated_residual_vec_round_bf16_f32_kernel<<<grid, block, 0, stream>>>(
        residual, update, gate, output, row_count, cols);
    return cudaGetLastError();
}

static __global__ void makepad_cuda_add_f32_precise_kernel(
        const float * __restrict__ left,
        const float * __restrict__ right,
        float * __restrict__ output,
        uint32_t n) {
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx < n) {
        output[idx] = left[idx] + right[idx];
    }
}

static __global__ void makepad_cuda_mul_f32_precise_kernel(
        const float * __restrict__ left,
        const float * __restrict__ right,
        float * __restrict__ output,
        uint32_t n) {
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx < n) {
        output[idx] = left[idx] * right[idx];
    }
}

// out[r][c] = a[r][c] * vec[c]  (row-broadcast multiply)
static __global__ void makepad_cuda_mul_rows_vec_f32_kernel(
        const float * __restrict__ input,
        const float * __restrict__ vec,
        float * __restrict__ output,
        uint32_t row_count,
        uint32_t cols) {
    const size_t total = static_cast<size_t>(row_count) * cols;
    const size_t idx = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    if (idx < total) {
        output[idx] = input[idx] * vec[idx % cols];
    }
}

// Tanh-approximation GELU matching gelu_scalar in libs/diffusion and the
// Metal kernel (GELU_COEF_A = 0.044715, sqrt(2/pi) below).
static __global__ void makepad_cuda_gelu_f32_precise_kernel(
        const float * __restrict__ input,
        float * __restrict__ output,
        uint32_t n) {
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx < n) {
        const float x = input[idx];
        const float inner = 0.79788456080286535587989211986876f * x * (1.0f + 0.044715f * x * x);
        output[idx] = 0.5f * x * (1.0f + tanhf(inner));
    }
}

// Row softmax without the bf16 rounding of the ops.cu variant.
static __global__ void makepad_cuda_softmax_rows_precise_f32_kernel(
        const float * __restrict__ logits,
        float * __restrict__ probs,
        uint32_t row_count,
        uint32_t row_stride,
        uint32_t seq_len) {
    const uint32_t row = blockIdx.x;
    if (row >= row_count) {
        return;
    }
    const float * row_in = logits + static_cast<size_t>(row) * row_stride;
    float * row_out = probs + static_cast<size_t>(row) * row_stride;

    float max_value = -CUDART_INF_F;
    for (uint32_t idx = threadIdx.x; idx < seq_len; idx += blockDim.x) {
        const float value = row_in[idx];
        max_value = value > max_value ? value : max_value;
    }
    max_value = makepad_cuda_diff_block_reduce_max(max_value);
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
    sum = makepad_cuda_diff_block_reduce_sum(sum);
    if (threadIdx.x == 0) {
        shared_sum = sum;
    }
    __syncthreads();

    for (uint32_t idx = threadIdx.x; idx < seq_len; idx += blockDim.x) {
        row_out[idx] = expf(row_in[idx] - shared_max) / shared_sum;
    }
}

// HY-Motion's packed MMDiT mask after padding rows have been removed:
//   motion query -> motion keys within +/- band_radius and every text key
//   text query   -> text keys only
// Scores are laid out [head][query][key], hence query = row % seq_len.
static __global__ void makepad_cuda_softmax_rows_motion_text_f32_kernel(
        const float * __restrict__ logits,
        float * __restrict__ probs,
        uint32_t row_count,
        uint32_t row_stride,
        uint32_t seq_len,
        uint32_t motion_tokens,
        uint32_t band_radius) {
    const uint32_t row = blockIdx.x;
    if (row >= row_count) {
        return;
    }
    const uint32_t query = row % seq_len;
    const float * row_in = logits + static_cast<size_t>(row) * row_stride;
    float * row_out = probs + static_cast<size_t>(row) * row_stride;

    float max_value = -CUDART_INF_F;
    for (uint32_t key = threadIdx.x; key < seq_len; key += blockDim.x) {
        const bool allowed = query < motion_tokens
            ? (key >= motion_tokens ||
               (key + band_radius >= query && key <= query + band_radius))
            : key >= motion_tokens;
        if (allowed) {
            const float value = row_in[key];
            max_value = value > max_value ? value : max_value;
        }
    }
    max_value = makepad_cuda_diff_block_reduce_max(max_value);
    __shared__ float shared_max;
    __shared__ float shared_sum;
    if (threadIdx.x == 0) {
        shared_max = max_value;
    }
    __syncthreads();

    float sum = 0.0f;
    for (uint32_t key = threadIdx.x; key < seq_len; key += blockDim.x) {
        const bool allowed = query < motion_tokens
            ? (key >= motion_tokens ||
               (key + band_radius >= query && key <= query + band_radius))
            : key >= motion_tokens;
        if (allowed) {
            sum += expf(row_in[key] - shared_max);
        }
    }
    sum = makepad_cuda_diff_block_reduce_sum(sum);
    if (threadIdx.x == 0) {
        shared_sum = sum;
    }
    __syncthreads();

    for (uint32_t key = threadIdx.x; key < seq_len; key += blockDim.x) {
        const bool allowed = query < motion_tokens
            ? (key >= motion_tokens ||
               (key + band_radius >= query && key <= query + band_radius))
            : key >= motion_tokens;
        row_out[key] = allowed ? expf(row_in[key] - shared_max) / shared_sum : 0.0f;
    }
}

// Bidirectional sliding-window softmax: keep |query-key| <= window.
// Scores are laid out [head][query][key], hence query = row % seq_len.
static __global__ void makepad_cuda_softmax_rows_sliding_f32_kernel(
        const float * __restrict__ logits,
        float * __restrict__ probs,
        uint32_t row_count,
        uint32_t row_stride,
        uint32_t seq_len,
        uint32_t window) {
    const uint32_t row = blockIdx.x;
    if (row >= row_count) {
        return;
    }
    const uint32_t query = row % seq_len;
    const float * row_in = logits + static_cast<size_t>(row) * row_stride;
    float * row_out = probs + static_cast<size_t>(row) * row_stride;
    const uint32_t lo = query > window ? query - window : 0u;
    const uint32_t hi = query + window < seq_len ? query + window : seq_len - 1u;

    float max_value = -CUDART_INF_F;
    for (uint32_t key = threadIdx.x; key < seq_len; key += blockDim.x) {
        if (key >= lo && key <= hi) {
            const float value = row_in[key];
            max_value = value > max_value ? value : max_value;
        }
    }
    max_value = makepad_cuda_diff_block_reduce_max(max_value);
    __shared__ float shared_max;
    __shared__ float shared_sum;
    if (threadIdx.x == 0) {
        shared_max = max_value;
    }
    __syncthreads();

    float sum = 0.0f;
    for (uint32_t key = threadIdx.x; key < seq_len; key += blockDim.x) {
        if (key >= lo && key <= hi) {
            sum += expf(row_in[key] - shared_max);
        }
    }
    sum = makepad_cuda_diff_block_reduce_sum(sum);
    if (threadIdx.x == 0) {
        shared_sum = sum > 1e-30f ? sum : 1e-30f;
    }
    __syncthreads();

    for (uint32_t key = threadIdx.x; key < seq_len; key += blockDim.x) {
        row_out[key] = (key >= lo && key <= hi)
            ? expf(row_in[key] - shared_max) / shared_sum
            : 0.0f;
    }
}

// Planar (channel-major, [c][y][x]) stride-1 "same" conv2d matching
// apply_conv2d_spatial in libs/diffusion/src/flux_vae.rs. Weights are laid
// out [out_c][in_c][kh][kw]; bias is per out channel.
static __global__ void makepad_cuda_conv2d_planar_f32_kernel(
        const float * __restrict__ input,
        const float * __restrict__ weights,
        const float * __restrict__ bias,
        float * __restrict__ output,
        uint32_t width,
        uint32_t height,
        uint32_t in_channels,
        uint32_t out_channels,
        uint32_t kw,
        uint32_t kh,
        uint32_t pad_x,
        uint32_t pad_y) {
    const uint32_t x = blockIdx.x * blockDim.x + threadIdx.x;
    const uint32_t y = blockIdx.y * blockDim.y + threadIdx.y;
    const uint32_t oc = blockIdx.z;
    if (x >= width || y >= height || oc >= out_channels) {
        return;
    }
    const size_t plane = static_cast<size_t>(width) * height;
    float acc = bias[oc];
    const float * w_oc = weights + static_cast<size_t>(oc) * in_channels * kh * kw;
    for (uint32_t ic = 0; ic < in_channels; ic++) {
        const float * in_plane = input + static_cast<size_t>(ic) * plane;
        const float * w_ic = w_oc + static_cast<size_t>(ic) * kh * kw;
        for (uint32_t ky = 0; ky < kh; ky++) {
            const uint32_t src_y = y + ky;
            if (src_y < pad_y || src_y - pad_y >= height) {
                continue;
            }
            const uint32_t in_y = src_y - pad_y;
            for (uint32_t kx = 0; kx < kw; kx++) {
                const uint32_t src_x = x + kx;
                if (src_x < pad_x || src_x - pad_x >= width) {
                    continue;
                }
                const uint32_t in_x = src_x - pad_x;
                acc += in_plane[static_cast<size_t>(in_y) * width + in_x]
                    * w_ic[ky * kw + kx];
            }
        }
    }
    output[static_cast<size_t>(oc) * plane + static_cast<size_t>(y) * width + x] = acc;
}

static __global__ void makepad_cuda_conv2d_planar_strided_f32_kernel(
        const float * __restrict__ input,
        const float * __restrict__ weights,
        const float * __restrict__ bias,
        float * __restrict__ output,
        uint32_t in_width,
        uint32_t in_height,
        uint32_t out_width,
        uint32_t out_height,
        uint32_t in_channels,
        uint32_t out_channels,
        uint32_t kw,
        uint32_t kh,
        uint32_t pad_x,
        uint32_t pad_y,
        uint32_t stride_x,
        uint32_t stride_y) {
    const uint32_t x = blockIdx.x * blockDim.x + threadIdx.x;
    const uint32_t y = blockIdx.y * blockDim.y + threadIdx.y;
    const uint32_t oc = blockIdx.z;
    if (x >= out_width || y >= out_height || oc >= out_channels) {
        return;
    }
    const size_t in_plane = static_cast<size_t>(in_width) * in_height;
    const size_t out_plane = static_cast<size_t>(out_width) * out_height;
    float acc = bias[oc];
    const float * w_oc = weights + static_cast<size_t>(oc) * in_channels * kh * kw;
    for (uint32_t ic = 0; ic < in_channels; ic++) {
        const float * in_plane_ptr = input + static_cast<size_t>(ic) * in_plane;
        const float * w_ic = w_oc + static_cast<size_t>(ic) * kh * kw;
        for (uint32_t ky = 0; ky < kh; ky++) {
            const int32_t in_y = static_cast<int32_t>(y * stride_y + ky) - static_cast<int32_t>(pad_y);
            if (in_y < 0 || in_y >= static_cast<int32_t>(in_height)) {
                continue;
            }
            for (uint32_t kx = 0; kx < kw; kx++) {
                const int32_t in_x = static_cast<int32_t>(x * stride_x + kx) - static_cast<int32_t>(pad_x);
                if (in_x < 0 || in_x >= static_cast<int32_t>(in_width)) {
                    continue;
                }
                acc += in_plane_ptr[static_cast<size_t>(in_y) * in_width + static_cast<size_t>(in_x)]
                    * w_ic[ky * kw + kx];
            }
        }
    }
    output[static_cast<size_t>(oc) * out_plane + static_cast<size_t>(y) * out_width + x] = acc;
}

// Group norm over planar [c][y][x] data: pass 1 computes per-group mean and
// inverse stddev (biased variance, f64 accumulation to match the CPU
// reference), pass 2 normalizes with per-channel gamma/beta.
static __global__ void makepad_cuda_group_norm_planar_stats_kernel(
        const float * __restrict__ input,
        float * __restrict__ stats,   // [group] -> (mean, inv_std)
        uint32_t plane,               // width * height
        uint32_t channels_per_group,
        float eps) {
    const uint32_t group = blockIdx.x;
    const size_t group_elems = static_cast<size_t>(plane) * channels_per_group;
    const float * group_in = input + static_cast<size_t>(group) * group_elems;
    double sum = 0.0;
    double sum_sq = 0.0;
    for (size_t idx = threadIdx.x; idx < group_elems; idx += blockDim.x) {
        const double value = static_cast<double>(group_in[idx]);
        sum += value;
        sum_sq += value * value;
    }
    // f64 block reduction via shared memory
    __shared__ double shared_sum[256];
    __shared__ double shared_sum_sq[256];
    shared_sum[threadIdx.x] = sum;
    shared_sum_sq[threadIdx.x] = sum_sq;
    __syncthreads();
    for (uint32_t stride = blockDim.x / 2; stride > 0; stride >>= 1) {
        if (threadIdx.x < stride) {
            shared_sum[threadIdx.x] += shared_sum[threadIdx.x + stride];
            shared_sum_sq[threadIdx.x] += shared_sum_sq[threadIdx.x + stride];
        }
        __syncthreads();
    }
    if (threadIdx.x == 0) {
        const double count = static_cast<double>(group_elems);
        const float mean = static_cast<float>(shared_sum[0] / count);
        const float variance = static_cast<float>(shared_sum_sq[0] / count) - mean * mean;
        stats[group * 2] = mean;
        stats[group * 2 + 1] = rsqrtf(variance + eps);
    }
}

static __global__ void makepad_cuda_group_norm_planar_apply_kernel(
        const float * __restrict__ input,
        const float * __restrict__ gamma,
        const float * __restrict__ beta,
        const float * __restrict__ stats,
        float * __restrict__ output,
        uint32_t plane,
        uint32_t channels,
        uint32_t channels_per_group) {
    const size_t total = static_cast<size_t>(plane) * channels;
    const size_t idx = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    if (idx >= total) {
        return;
    }
    const uint32_t channel = static_cast<uint32_t>(idx / plane);
    const uint32_t group = channel / channels_per_group;
    const float mean = stats[group * 2];
    const float inv_std = stats[group * 2 + 1];
    output[idx] = (input[idx] - mean) * inv_std * gamma[channel] + beta[channel];
}

static __global__ void makepad_cuda_silu_f32_precise_kernel(
        const float * __restrict__ input,
        float * __restrict__ output,
        uint32_t n) {
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx < n) {
        const float x = input[idx];
        output[idx] = x / (1.0f + expf(-x));
    }
}

static __global__ void makepad_cuda_f32_to_f16_kernel(
        const float * __restrict__ input,
        uint16_t * __restrict__ output,
        uint32_t n) {
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx < n) {
        const half h = __float2half_rn(input[idx]);
        output[idx] = *reinterpret_cast<const uint16_t *>(&h);
    }
}

extern "C" cudaError_t makepad_cuda_layer_norm_mul_add_f32(
        const float * input,
        const float * gamma,
        const float * beta,
        float * output,
        uint32_t row_count,
        uint32_t cols,
        float eps,
        float gamma_add,
        cudaStream_t stream) {
    if (row_count == 0 || cols == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid(row_count, 1, 1);
    makepad_cuda_layer_norm_mul_add_f32_kernel<<<grid, block, 0, stream>>>(
        input, gamma, beta, output, row_count, cols, eps, gamma_add);
    return cudaGetLastError();
}

// PyTorch 2.7 CUDA LayerNorm fast path, used as the numeric oracle by the
// released SkinTokens checkpoint. PyTorch vectorizes four f32 values per
// lane, accumulates row moments with online Welford, combines four warps in a
// fixed tree, and evaluates gamma * (rstd * (x - mean)) + beta in precisely
// that association. The generic two-pass LayerNorm above intentionally stays
// unchanged for existing Makepad models.
struct makepad_cuda_welford_ln {
    float mean;
    float sigma2;
    float count;
};

static __device__ __forceinline__ makepad_cuda_welford_ln
makepad_cuda_welford_ln_online(
        float value,
        const makepad_cuda_welford_ln &current) {
    const float delta = value - current.mean;
    const float new_count = current.count + 1.0f;
    const float new_mean = current.mean + delta * (1.0f / new_count);
    return {new_mean, current.sigma2 + delta * (value - new_mean), new_count};
}

static __device__ __forceinline__ makepad_cuda_welford_ln
makepad_cuda_welford_ln_combine(
        const makepad_cuda_welford_ln data_b,
        const makepad_cuda_welford_ln data_a) {
    const float delta = data_b.mean - data_a.mean;
    const float count = data_a.count + data_b.count;
    if (count <= 0.0f) {
        return {0.0f, 0.0f, 0.0f};
    }
    const float coefficient = 1.0f / count;
    const float n_a = data_a.count * coefficient;
    const float n_b = data_b.count * coefficient;
    const float mean = n_a * data_a.mean + n_b * data_b.mean;
    const float sigma2 = data_a.sigma2 + data_b.sigma2
        + delta * delta * data_a.count * n_b;
    return {mean, sigma2, count};
}

static __global__ void makepad_cuda_layer_norm_pytorch_f32_kernel(
        const float * __restrict__ input,
        const float * __restrict__ gamma,
        const float * __restrict__ beta,
        float * __restrict__ output,
        uint32_t cols,
        float eps) {
    // Launch contract is exactly PyTorch's dim3(warp_size, 4, 1).
    const uint32_t row = blockIdx.x;
    const uint32_t flat_thread = threadIdx.x + threadIdx.y * blockDim.x;
    const uint32_t flat_threads = blockDim.x * blockDim.y;
    const uint32_t vector_count = cols / 4;
    const float4 * input4 = reinterpret_cast<const float4 *>(
        input + static_cast<size_t>(row) * cols);
    const float4 * gamma4 = reinterpret_cast<const float4 *>(gamma);
    const float4 * beta4 = reinterpret_cast<const float4 *>(beta);
    float4 * output4 = reinterpret_cast<float4 *>(
        output + static_cast<size_t>(row) * cols);

    makepad_cuda_welford_ln wd{0.0f, 0.0f, 0.0f};
    for (uint32_t index = flat_thread; index < vector_count; index += flat_threads) {
        const float4 data = input4[index];
        wd = makepad_cuda_welford_ln_online(data.x, wd);
        wd = makepad_cuda_welford_ln_online(data.y, wd);
        wd = makepad_cuda_welford_ln_online(data.z, wd);
        wd = makepad_cuda_welford_ln_online(data.w, wd);
    }
    for (int offset = 16; offset > 0; offset >>= 1) {
        makepad_cuda_welford_ln other{
            __shfl_down_sync(0xffffffffu, wd.mean, offset),
            __shfl_down_sync(0xffffffffu, wd.sigma2, offset),
            __shfl_down_sync(0xffffffffu, wd.count, offset),
        };
        wd = makepad_cuda_welford_ln_combine(wd, other);
    }

    // PyTorch allocates `warps * 3/2` floats and uses this deliberately
    // compact layout for the pairwise inter-warp Welford tree.
    __shared__ float stats[6];
    float * mean_sigma = stats;
    float * counts = stats + blockDim.y;
    for (int offset = blockDim.y / 2; offset > 0; offset >>= 1) {
        if (threadIdx.x == 0 && threadIdx.y >= offset && threadIdx.y < 2 * offset) {
            const int write_y = threadIdx.y - offset;
            mean_sigma[2 * write_y] = wd.mean;
            mean_sigma[2 * write_y + 1] = wd.sigma2;
            counts[write_y] = wd.count;
        }
        __syncthreads();
        if (threadIdx.x == 0 && threadIdx.y < offset) {
            const makepad_cuda_welford_ln other{
                mean_sigma[2 * threadIdx.y],
                mean_sigma[2 * threadIdx.y + 1],
                counts[threadIdx.y],
            };
            wd = makepad_cuda_welford_ln_combine(wd, other);
        }
        __syncthreads();
    }
    if (threadIdx.x == 0 && threadIdx.y == 0) {
        mean_sigma[0] = wd.mean;
        mean_sigma[1] = wd.sigma2 / static_cast<float>(cols);
    }
    __syncthreads();

    const float mean = mean_sigma[0];
    const float rstd = rsqrtf(mean_sigma[1] + eps);
    for (uint32_t index = flat_thread; index < vector_count; index += flat_threads) {
        const float4 data = input4[index];
        const float4 scale = gamma4[index];
        const float4 shift = beta4[index];
        float4 result;
        result.x = scale.x * (rstd * (data.x - mean)) + shift.x;
        result.y = scale.y * (rstd * (data.y - mean)) + shift.y;
        result.z = scale.z * (rstd * (data.z - mean)) + shift.z;
        result.w = scale.w * (rstd * (data.w - mean)) + shift.w;
        output4[index] = result;
    }
}

extern "C" cudaError_t makepad_cuda_layer_norm_pytorch_f32(
        const float * input,
        const float * gamma,
        const float * beta,
        float * output,
        uint32_t row_count,
        uint32_t cols,
        float eps,
        cudaStream_t stream) {
    if (row_count == 0 || cols == 0) {
        return cudaSuccess;
    }
    if ((cols & 3u) != 0) {
        return cudaErrorInvalidValue;
    }
    const dim3 block(32, 4, 1);
    const dim3 grid(row_count, 1, 1);
    makepad_cuda_layer_norm_pytorch_f32_kernel<<<grid, block, 0, stream>>>(
        input, gamma, beta, output, cols, eps);
    return cudaGetLastError();
}

static __global__ void makepad_cuda_rpb_expand_f32_kernel(
        const float * ry,
        const float * rx,
        float * bias,
        int queries,
        int height,
        int width,
        int heads) {
    int pix = (int)(blockIdx.x * blockDim.x + threadIdx.x);
    int q = (int)blockIdx.y;
    int h = (int)blockIdx.z;
    int hw = height * width;
    if (pix >= hw || q >= queries || h >= heads) {
        return;
    }
    int y = pix / width;
    int x = pix - y * width;
    float v = ry[(q * height + y) * heads + h] + rx[(q * width + x) * heads + h];
    int q1 = queries + 1;
    bias[(h * q1 + (q + 1)) * hw + pix] = v;
}

extern "C" cudaError_t makepad_cuda_rpb_expand_f32(
        const float * ry,
        const float * rx,
        float * bias,
        uint32_t queries,
        uint32_t height,
        uint32_t width,
        uint32_t heads,
        cudaStream_t stream) {
    if (queries == 0 || height == 0 || width == 0 || heads == 0) {
        return cudaSuccess;
    }
    size_t bytes = (size_t)heads * (size_t)(queries + 1) * (size_t)height * (size_t)width * sizeof(float);
    cudaError_t st = cudaMemsetAsync(bias, 0, bytes, stream);
    if (st != cudaSuccess) {
        return st;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid((height * width + block.x - 1) / block.x, queries, heads);
    makepad_cuda_rpb_expand_f32_kernel<<<grid, block, 0, stream>>>(
        ry, rx, bias, (int)queries, (int)height, (int)width, (int)heads);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_cuda_add_f32_precise(
        const float * left,
        const float * right,
        float * output,
        uint32_t n,
        cudaStream_t stream) {
    if (n == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid((n + block.x - 1) / block.x, 1, 1);
    makepad_cuda_add_f32_precise_kernel<<<grid, block, 0, stream>>>(left, right, output, n);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_cuda_mul_f32_precise(
        const float * left,
        const float * right,
        float * output,
        uint32_t n,
        cudaStream_t stream) {
    if (n == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid((n + block.x - 1) / block.x, 1, 1);
    makepad_cuda_mul_f32_precise_kernel<<<grid, block, 0, stream>>>(left, right, output, n);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_cuda_mul_rows_vec_f32(
        const float * input,
        const float * vec,
        float * output,
        uint32_t row_count,
        uint32_t cols,
        cudaStream_t stream) {
    const size_t total = static_cast<size_t>(row_count) * cols;
    if (total == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<unsigned int>((total + block.x - 1) / block.x), 1, 1);
    makepad_cuda_mul_rows_vec_f32_kernel<<<grid, block, 0, stream>>>(
        input, vec, output, row_count, cols);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_cuda_gelu_f32_precise(
        const float * input,
        float * output,
        uint32_t n,
        cudaStream_t stream) {
    if (n == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid((n + block.x - 1) / block.x, 1, 1);
    makepad_cuda_gelu_f32_precise_kernel<<<grid, block, 0, stream>>>(input, output, n);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_cuda_softmax_rows_precise_f32(
        const float * logits,
        float * probs,
        uint32_t row_count,
        uint32_t row_stride,
        uint32_t seq_len,
        cudaStream_t stream) {
    if (row_count == 0 || seq_len == 0) {
        return cudaSuccess;
    }
    const dim3 block(seq_len < 1024 ? 256 : 1024, 1, 1);
    const dim3 grid(row_count, 1, 1);
    makepad_cuda_softmax_rows_precise_f32_kernel<<<grid, block, 0, stream>>>(
        logits, probs, row_count, row_stride, seq_len);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_cuda_softmax_rows_motion_text_f32(
        const float * logits,
        float * probs,
        uint32_t row_count,
        uint32_t row_stride,
        uint32_t seq_len,
        uint32_t motion_tokens,
        uint32_t band_radius,
        cudaStream_t stream) {
    if (row_count == 0 || seq_len == 0) {
        return cudaSuccess;
    }
    if (motion_tokens == 0 || motion_tokens >= seq_len) {
        return cudaErrorInvalidValue;
    }
    const dim3 block(seq_len < 1024 ? 256 : 1024, 1, 1);
    const dim3 grid(row_count, 1, 1);
    makepad_cuda_softmax_rows_motion_text_f32_kernel<<<grid, block, 0, stream>>>(
        logits, probs, row_count, row_stride, seq_len, motion_tokens, band_radius);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_cuda_softmax_rows_sliding_f32(
        const float * logits,
        float * probs,
        uint32_t row_count,
        uint32_t row_stride,
        uint32_t seq_len,
        uint32_t window,
        cudaStream_t stream) {
    if (row_count == 0 || seq_len == 0) {
        return cudaSuccess;
    }
    const dim3 block(seq_len < 1024 ? 256 : 1024, 1, 1);
    const dim3 grid(row_count, 1, 1);
    makepad_cuda_softmax_rows_sliding_f32_kernel<<<grid, block, 0, stream>>>(
        logits, probs, row_count, row_stride, seq_len, window);
    return cudaGetLastError();
}

static __global__ void makepad_cuda_snake_rows_f32_kernel(
        const float * __restrict__ input,
        const float * __restrict__ alpha,
        const float * __restrict__ inv_beta,
        float * __restrict__ output,
        uint32_t rows,
        uint32_t cols) {
    const size_t idx = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    const size_t n = static_cast<size_t>(rows) * cols;
    if (idx >= n) {
        return;
    }
    const uint32_t c = static_cast<uint32_t>(idx % cols);
    const float x = input[idx];
    const float sn = sinf(alpha[c] * x);
    output[idx] = x + inv_beta[c] * sn * sn;
}

extern "C" cudaError_t makepad_cuda_snake_rows_f32(
        const float * input,
        const float * alpha,
        const float * inv_beta,
        float * output,
        uint32_t rows,
        uint32_t cols,
        cudaStream_t stream) {
    const size_t n = static_cast<size_t>(rows) * cols;
    if (n == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<uint32_t>((n + 255) / 256), 1, 1);
    makepad_cuda_snake_rows_f32_kernel<<<grid, block, 0, stream>>>(
        input, alpha, inv_beta, output, rows, cols);
    return cudaGetLastError();
}

static __global__ void makepad_cuda_tconv_stitch_f32_kernel(
        const float * __restrict__ y_hi,
        const float * __restrict__ y_lo,
        float * __restrict__ output,
        uint32_t in_len,
        uint32_t out_len,
        uint32_t out_ch,
        uint32_t stride,
        uint32_t padding) {
    const size_t idx = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    const size_t n = static_cast<size_t>(out_len) * out_ch;
    if (idx >= n) {
        return;
    }
    const uint32_t t = static_cast<uint32_t>(idx / out_ch);
    const uint32_t o = static_cast<uint32_t>(idx % out_ch);
    const uint32_t tp = t + padding;
    const uint32_t src = tp / stride;
    const uint32_t r = tp % stride;
    float acc = 0.0f;
    if (src < in_len) {
        acc += y_hi[(static_cast<size_t>(src) * stride + r) * out_ch + o];
    }
    if (src > 0u && (src - 1u) < in_len) {
        acc += y_lo[(static_cast<size_t>(src - 1u) * stride + r) * out_ch + o];
    }
    output[idx] = acc;
}

extern "C" cudaError_t makepad_cuda_tconv_stitch_f32(
        const float * y_hi,
        const float * y_lo,
        float * output,
        uint32_t in_len,
        uint32_t out_len,
        uint32_t out_ch,
        uint32_t stride,
        uint32_t padding,
        cudaStream_t stream) {
    const size_t n = static_cast<size_t>(out_len) * out_ch;
    if (n == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<uint32_t>((n + 255) / 256), 1, 1);
    makepad_cuda_tconv_stitch_f32_kernel<<<grid, block, 0, stream>>>(
        y_hi, y_lo, output, in_len, out_len, out_ch, stride, padding);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_cuda_conv2d_planar_f32(
        const float * input,
        const float * weights,
        const float * bias,
        float * output,
        uint32_t width,
        uint32_t height,
        uint32_t in_channels,
        uint32_t out_channels,
        uint32_t kw,
        uint32_t kh,
        uint32_t pad_x,
        uint32_t pad_y,
        cudaStream_t stream) {
    if (width == 0 || height == 0 || out_channels == 0) {
        return cudaSuccess;
    }
    const dim3 block(16, 16, 1);
    const dim3 grid(
        (width + block.x - 1) / block.x,
        (height + block.y - 1) / block.y,
        out_channels);
    makepad_cuda_conv2d_planar_f32_kernel<<<grid, block, 0, stream>>>(
        input, weights, bias, output, width, height, in_channels, out_channels,
        kw, kh, pad_x, pad_y);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_cuda_conv2d_planar_strided_f32(
        const float * input,
        const float * weights,
        const float * bias,
        float * output,
        uint32_t in_width,
        uint32_t in_height,
        uint32_t out_width,
        uint32_t out_height,
        uint32_t in_channels,
        uint32_t out_channels,
        uint32_t kw,
        uint32_t kh,
        uint32_t pad_x,
        uint32_t pad_y,
        uint32_t stride_x,
        uint32_t stride_y,
        cudaStream_t stream) {
    if (out_width == 0 || out_height == 0 || out_channels == 0) {
        return cudaSuccess;
    }
    if (stride_x == 0 || stride_y == 0) {
        return cudaErrorInvalidValue;
    }
    const dim3 block(16, 16, 1);
    const dim3 grid(
        (out_width + block.x - 1) / block.x,
        (out_height + block.y - 1) / block.y,
        out_channels);
    makepad_cuda_conv2d_planar_strided_f32_kernel<<<grid, block, 0, stream>>>(
        input, weights, bias, output, in_width, in_height, out_width, out_height,
        in_channels, out_channels, kw, kh, pad_x, pad_y, stride_x, stride_y);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_cuda_group_norm_planar_f32(
        const float * input,
        const float * gamma,
        const float * beta,
        float * stats,
        float * output,
        uint32_t width,
        uint32_t height,
        uint32_t channels,
        uint32_t groups,
        float eps,
        cudaStream_t stream) {
    if (width == 0 || height == 0 || channels == 0 || groups == 0) {
        return cudaSuccess;
    }
    if (channels % groups != 0) {
        return cudaErrorInvalidValue;
    }
    const uint32_t plane = width * height;
    const uint32_t channels_per_group = channels / groups;
    makepad_cuda_group_norm_planar_stats_kernel<<<groups, 256, 0, stream>>>(
        input, stats, plane, channels_per_group, eps);
    const size_t total = static_cast<size_t>(plane) * channels;
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<unsigned int>((total + block.x - 1) / block.x), 1, 1);
    makepad_cuda_group_norm_planar_apply_kernel<<<grid, block, 0, stream>>>(
        input, gamma, beta, stats, output, plane, channels, channels_per_group);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_cuda_silu_f32_precise(
        const float * input,
        float * output,
        uint32_t n,
        cudaStream_t stream) {
    if (n == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid((n + block.x - 1) / block.x, 1, 1);
    makepad_cuda_silu_f32_precise_kernel<<<grid, block, 0, stream>>>(input, output, n);
    return cudaGetLastError();
}

static __global__ void makepad_cuda_f16_to_f32_precise_kernel(
        const __half * __restrict__ input,
        float * __restrict__ output,
        uint32_t n) {
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx < n) {
        output[idx] = __half2float(input[idx]);
    }
}

// out[r][c] = f32(input_f16[r][c]) + bias[c] — the f16-accumulate gemm's
// C-matrix convert and the bias broadcast in ONE pass (separate passes cost
// an extra full read+write of the f32 output).
static __global__ void makepad_cuda_f16_bias_to_f32_kernel(
        const __half * __restrict__ input,
        const float * __restrict__ bias,
        float * __restrict__ output,
        uint32_t row_count,
        uint32_t cols) {
    const size_t total = static_cast<size_t>(row_count) * cols;
    const size_t idx = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    if (idx < total) {
        output[idx] = __half2float(input[idx]) + bias[idx % cols];
    }
}

extern "C" cudaError_t makepad_cuda_f16_bias_to_f32(
        const uint16_t * input,
        const float * bias,
        float * output,
        uint32_t row_count,
        uint32_t cols,
        cudaStream_t stream) {
    const size_t total = static_cast<size_t>(row_count) * cols;
    if (total == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<unsigned int>((total + block.x - 1) / block.x), 1, 1);
    makepad_cuda_f16_bias_to_f32_kernel<<<grid, block, 0, stream>>>(
        reinterpret_cast<const __half *>(input), bias, output, row_count, cols);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_cuda_f16_to_f32_precise(
        const uint16_t * input,
        float * output,
        uint32_t n,
        cudaStream_t stream) {
    if (n == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid((n + block.x - 1) / block.x, 1, 1);
    makepad_cuda_f16_to_f32_precise_kernel<<<grid, block, 0, stream>>>(
        reinterpret_cast<const __half *>(input), output, n);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_cuda_f32_to_f16(
        const float * input,
        uint16_t * output,
        uint32_t n,
        cudaStream_t stream) {
    if (n == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid((n + block.x - 1) / block.x, 1, 1);
    makepad_cuda_f32_to_f16_kernel<<<grid, block, 0, stream>>>(input, output, n);
    return cudaGetLastError();
}

// out[r][c] = a[r][c] + vec[c]  (row-broadcast add; bias application)
static __global__ void makepad_cuda_add_rows_vec_f32_kernel(
        const float * __restrict__ input,
        const float * __restrict__ vec,
        float * __restrict__ output,
        uint32_t row_count,
        uint32_t cols) {
    const size_t total = static_cast<size_t>(row_count) * cols;
    const size_t idx = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    if (idx < total) {
        output[idx] = input[idx] + vec[idx % cols];
    }
}

extern "C" cudaError_t makepad_cuda_add_rows_vec_f32(
        const float * input,
        const float * vec,
        float * output,
        uint32_t row_count,
        uint32_t cols,
        cudaStream_t stream) {
    const size_t total = static_cast<size_t>(row_count) * cols;
    if (total == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<unsigned int>((total + block.x - 1) / block.x), 1, 1);
    makepad_cuda_add_rows_vec_f32_kernel<<<grid, block, 0, stream>>>(
        input, vec, output, row_count, cols);
    return cudaGetLastError();
}

// Interleaved-pair RoPE matching apply_flux_rope_heads in
// libs/diffusion/src/flux_transformer.rs: data is token-major
// [token][head][dim]; cos/sin tables are [token][half_dim] with
// half_dim = dim / 2. Pairs are (2*p, 2*p+1) within each head.
static __global__ void makepad_cuda_rope_interleaved_f32_kernel(
        const float * __restrict__ input,
        const float * __restrict__ cos_table,
        const float * __restrict__ sin_table,
        float * __restrict__ output,
        uint32_t token_count,
        uint32_t head_count,
        uint32_t half_dim) {
    const uint32_t token = blockIdx.x;
    const uint32_t head = blockIdx.y;
    if (token >= token_count || head >= head_count) {
        return;
    }
    const size_t base = (static_cast<size_t>(token) * head_count + head)
        * (static_cast<size_t>(half_dim) * 2);
    const size_t table_base = static_cast<size_t>(token) * half_dim;
    for (uint32_t pair = threadIdx.x; pair < half_dim; pair += blockDim.x) {
        const size_t even = base + static_cast<size_t>(pair) * 2;
        const size_t odd = even + 1;
        const float cos_v = cos_table[table_base + pair];
        const float sin_v = sin_table[table_base + pair];
        const float x0 = input[even];
        const float x1 = input[odd];
        output[even] = x0 * cos_v - x1 * sin_v;
        output[odd] = x0 * sin_v + x1 * cos_v;
    }
}

extern "C" cudaError_t makepad_cuda_rope_interleaved_f32(
        const float * input,
        const float * cos_table,
        const float * sin_table,
        float * output,
        uint32_t token_count,
        uint32_t head_count,
        uint32_t half_dim,
        cudaStream_t stream) {
    if (token_count == 0 || head_count == 0 || half_dim == 0) {
        return cudaSuccess;
    }
    const dim3 block(half_dim < 128 ? 32 : 128, 1, 1);
    const dim3 grid(token_count, head_count, 1);
    makepad_cuda_rope_interleaved_f32_kernel<<<grid, block, 0, stream>>>(
        input, cos_table, sin_table, output, token_count, head_count, half_dim);
    return cudaGetLastError();
}

// --- f16 activation-spine kernels -----------------------------------------
// The dense gemms accumulate in f16 and the attention kernels consume f16, so
// activations that only ever flow between those two can stay f16 end to end:
// qkv C -> slice -> rms -> concat -> rope -> attention, and the mlp.0 -> gelu
// -> mlp.2 bridge. These variants keep all arithmetic in f32 and only change
// the storage type — the values were getting rounded to f16 at the next gemm
// input anyway, so the numerics class is unchanged while the convert passes
// and half of the copy traffic disappear.

// Per-head RMS norm, f16 storage (f32 math, f32 weights).
static __global__ void makepad_cuda_rms_norm_rows_weighted_f16_kernel(
        const __half * __restrict__ input,
        const float * __restrict__ weights_f32,
        __half * __restrict__ output,
        uint32_t row_count,
        uint32_t row_stride,
        uint32_t n,
        float eps) {
    const uint32_t row = blockIdx.x;
    if (row >= row_count) {
        return;
    }
    const __half * row_in = input + static_cast<size_t>(row) * row_stride;
    __half * row_out = output + static_cast<size_t>(row) * row_stride;
    float sum = 0.0f;
    for (uint32_t idx = threadIdx.x; idx < n; idx += blockDim.x) {
        const float v = __half2float(row_in[idx]);
        sum += v * v;
    }
    sum = makepad_cuda_diff_block_reduce_sum(sum);
    __shared__ float inv_rms;
    if (threadIdx.x == 0) {
        inv_rms = rsqrtf(sum / static_cast<float>(n) + eps);
    }
    __syncthreads();
    for (uint32_t idx = threadIdx.x; idx < n; idx += blockDim.x) {
        row_out[idx] = __float2half_rn(
            __half2float(row_in[idx]) * inv_rms * weights_f32[idx]);
    }
}

extern "C" cudaError_t makepad_cuda_rms_norm_rows_weighted_f16(
        const uint16_t * input,
        const float * weights_f32,
        uint16_t * output,
        uint32_t row_count,
        uint32_t row_stride,
        uint32_t n,
        float eps,
        cudaStream_t stream) {
    if (row_count == 0 || n == 0) {
        return cudaSuccess;
    }
    const dim3 block(n < 256 ? 32 : 256, 1, 1);
    makepad_cuda_rms_norm_rows_weighted_f16_kernel<<<row_count, block, 0, stream>>>(
        reinterpret_cast<const __half *>(input), weights_f32,
        reinterpret_cast<__half *>(output), row_count, row_stride, n, eps);
    return cudaGetLastError();
}

// Interleaved-pair RoPE, f16 storage (f32 tables and math).
static __global__ void makepad_cuda_rope_interleaved_f16_kernel(
        const __half * __restrict__ input,
        const float * __restrict__ cos_table,
        const float * __restrict__ sin_table,
        __half * __restrict__ output,
        uint32_t token_count,
        uint32_t head_count,
        uint32_t half_dim) {
    const uint32_t token = blockIdx.x;
    const uint32_t head = blockIdx.y;
    if (token >= token_count || head >= head_count) {
        return;
    }
    const size_t base = (static_cast<size_t>(token) * head_count + head)
        * (static_cast<size_t>(half_dim) * 2);
    const size_t table_base = static_cast<size_t>(token) * half_dim;
    for (uint32_t pair = threadIdx.x; pair < half_dim; pair += blockDim.x) {
        const size_t even = base + static_cast<size_t>(pair) * 2;
        const size_t odd = even + 1;
        const float cos_v = cos_table[table_base + pair];
        const float sin_v = sin_table[table_base + pair];
        const float x0 = __half2float(input[even]);
        const float x1 = __half2float(input[odd]);
        output[even] = __float2half_rn(x0 * cos_v - x1 * sin_v);
        output[odd] = __float2half_rn(x0 * sin_v + x1 * cos_v);
    }
}

extern "C" cudaError_t makepad_cuda_rope_interleaved_f16(
        const uint16_t * input,
        const float * cos_table,
        const float * sin_table,
        uint16_t * output,
        uint32_t token_count,
        uint32_t head_count,
        uint32_t half_dim,
        cudaStream_t stream) {
    if (token_count == 0 || head_count == 0 || half_dim == 0) {
        return cudaSuccess;
    }
    const dim3 block(half_dim < 128 ? 32 : 128, 1, 1);
    const dim3 grid(token_count, head_count, 1);
    makepad_cuda_rope_interleaved_f16_kernel<<<grid, block, 0, stream>>>(
        reinterpret_cast<const __half *>(input), cos_table, sin_table,
        reinterpret_cast<__half *>(output), token_count, head_count, half_dim);
    return cudaGetLastError();
}

// LayerNorm + modulation with an f16 output (feeds the next linear's f16 A).
static __global__ void makepad_cuda_layer_norm_mul_add_f32_out16_kernel(
        const float * __restrict__ input,
        const float * __restrict__ gamma,
        const float * __restrict__ beta,
        __half * __restrict__ output,
        uint32_t row_count,
        uint32_t cols,
        float eps,
        float gamma_add) {
    const uint32_t row = blockIdx.x;
    if (row >= row_count) {
        return;
    }
    const float * row_in = input + static_cast<size_t>(row) * cols;
    __half * row_out = output + static_cast<size_t>(row) * cols;

    float sum = 0.0f;
    for (uint32_t idx = threadIdx.x; idx < cols; idx += blockDim.x) {
        sum += row_in[idx];
    }
    sum = makepad_cuda_diff_block_reduce_sum(sum);
    __shared__ float shared_mean;
    __shared__ float shared_inv;
    if (threadIdx.x == 0) {
        shared_mean = sum / static_cast<float>(cols);
    }
    __syncthreads();

    const float mean = shared_mean;
    float sq_sum = 0.0f;
    for (uint32_t idx = threadIdx.x; idx < cols; idx += blockDim.x) {
        const float centered = row_in[idx] - mean;
        sq_sum += centered * centered;
    }
    sq_sum = makepad_cuda_diff_block_reduce_sum(sq_sum);
    if (threadIdx.x == 0) {
        shared_inv = rsqrtf(sq_sum / static_cast<float>(cols) + eps);
    }
    __syncthreads();

    const float inv = shared_inv;
    for (uint32_t idx = threadIdx.x; idx < cols; idx += blockDim.x) {
        row_out[idx] = __float2half_rn(
            (row_in[idx] - mean) * inv * (gamma[idx] + gamma_add) + beta[idx]);
    }
}

extern "C" cudaError_t makepad_cuda_layer_norm_mul_add_f32_out16(
        const float * input,
        const float * gamma,
        const float * beta,
        uint16_t * output,
        uint32_t row_count,
        uint32_t cols,
        float eps,
        float gamma_add,
        cudaStream_t stream) {
    if (row_count == 0 || cols == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid(row_count, 1, 1);
    makepad_cuda_layer_norm_mul_add_f32_out16_kernel<<<grid, block, 0, stream>>>(
        input, gamma, beta, reinterpret_cast<__half *>(output), row_count, cols,
        eps, gamma_add);
    return cudaGetLastError();
}

// Tanh-approximation GELU on f16 storage; optional f32 bias folded in (the
// mlp.0 gemm defers its bias here so its C never leaves f16).
static __global__ void makepad_cuda_gelu_f16_kernel(
        const __half * __restrict__ input,
        const float * __restrict__ bias,
        __half * __restrict__ output,
        uint32_t row_count,
        uint32_t cols) {
    const size_t total = static_cast<size_t>(row_count) * cols;
    const size_t idx = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    if (idx < total) {
        float x = __half2float(input[idx]);
        if (bias != nullptr) {
            x += bias[idx % cols];
        }
        const float inner = 0.79788456080286535587989211986876f * x * (1.0f + 0.044715f * x * x);
        output[idx] = __float2half_rn(0.5f * x * (1.0f + tanhf(inner)));
    }
}

extern "C" cudaError_t makepad_cuda_gelu_f16(
        const uint16_t * input,
        const float * bias,
        uint16_t * output,
        uint32_t row_count,
        uint32_t cols,
        cudaStream_t stream) {
    const size_t total = static_cast<size_t>(row_count) * cols;
    if (total == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<unsigned int>((total + block.x - 1) / block.x), 1, 1);
    makepad_cuda_gelu_f16_kernel<<<grid, block, 0, stream>>>(
        reinterpret_cast<const __half *>(input), bias,
        reinterpret_cast<__half *>(output), row_count, cols);
    return cudaGetLastError();
}

// In-place f32-bias broadcast onto an f16 C matrix (linear1's whole-row bias
// before the f16 qkv/mlp consumers split it).
static __global__ void makepad_cuda_f16_bias_inplace_kernel(
        __half * __restrict__ data,
        const float * __restrict__ bias,
        uint32_t row_count,
        uint32_t cols) {
    const size_t total = static_cast<size_t>(row_count) * cols;
    const size_t idx = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    if (idx < total) {
        data[idx] = __float2half_rn(__half2float(data[idx]) + bias[idx % cols]);
    }
}

extern "C" cudaError_t makepad_cuda_f16_bias_inplace(
        uint16_t * data,
        const float * bias,
        uint32_t row_count,
        uint32_t cols,
        cudaStream_t stream) {
    const size_t total = static_cast<size_t>(row_count) * cols;
    if (total == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<unsigned int>((total + block.x - 1) / block.x), 1, 1);
    makepad_cuda_f16_bias_inplace_kernel<<<grid, block, 0, stream>>>(
        reinterpret_cast<__half *>(data), bias, row_count, cols);
    return cudaGetLastError();
}

// Strided rows*cols block copy: dst[r*dst_stride + c] = src[r*src_stride + c].
// Column/row offsets are folded into the base pointers by the caller, so this
// one kernel backs slice_cols / concat_cols / slice_rows / concat_rows.
static __global__ void makepad_cuda_copy_submatrix_f32_kernel(
        const float * __restrict__ src,
        float * __restrict__ dst,
        uint32_t src_stride,
        uint32_t dst_stride,
        uint32_t row_count,
        uint32_t cols) {
    const size_t total = static_cast<size_t>(row_count) * cols;
    const size_t idx = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    if (idx >= total) {
        return;
    }
    const uint32_t row = static_cast<uint32_t>(idx / cols);
    const uint32_t col = static_cast<uint32_t>(idx % cols);
    dst[static_cast<size_t>(row) * dst_stride + col] =
        src[static_cast<size_t>(row) * src_stride + col];
}

extern "C" cudaError_t makepad_cuda_copy_submatrix_f32(
        const float * src,
        float * dst,
        uint32_t src_stride,
        uint32_t dst_stride,
        uint32_t row_count,
        uint32_t cols,
        cudaStream_t stream) {
    const size_t total = static_cast<size_t>(row_count) * cols;
    if (total == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<unsigned int>((total + block.x - 1) / block.x), 1, 1);
    makepad_cuda_copy_submatrix_f32_kernel<<<grid, block, 0, stream>>>(
        src, dst, src_stride, dst_stride, row_count, cols);
    return cudaGetLastError();
}

// Nearest-neighbour 2x upsample on planar [c][y][x] data, matching
// upscale_nearest(factor=2) in libs/diffusion/src/flux_vae.rs.
static __global__ void makepad_cuda_upsample2x_planar_f32_kernel(
        const float * __restrict__ src,
        float * __restrict__ dst,
        uint32_t width,
        uint32_t height,
        uint32_t channels) {
    const uint32_t out_width = width * 2;
    const uint32_t out_height = height * 2;
    const size_t total = static_cast<size_t>(out_width) * out_height * channels;
    const size_t idx = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    if (idx >= total) {
        return;
    }
    const uint32_t x = static_cast<uint32_t>(idx % out_width);
    const size_t rest = idx / out_width;
    const uint32_t y = static_cast<uint32_t>(rest % out_height);
    const uint32_t c = static_cast<uint32_t>(rest / out_height);
    const size_t src_idx = static_cast<size_t>(x / 2)
        + static_cast<size_t>(width) * ((y / 2) + static_cast<size_t>(height) * c);
    dst[idx] = src[src_idx];
}

extern "C" cudaError_t makepad_cuda_upsample2x_planar_f32(
        const float * src,
        float * dst,
        uint32_t width,
        uint32_t height,
        uint32_t channels,
        cudaStream_t stream) {
    const size_t total = static_cast<size_t>(width) * 2 * height * 2 * channels;
    if (total == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<unsigned int>((total + block.x - 1) / block.x), 1, 1);
    makepad_cuda_upsample2x_planar_f32_kernel<<<grid, block, 0, stream>>>(
        src, dst, width, height, channels);
    return cudaGetLastError();
}

// Zero-pad each [y][x] plane by (pad_x, pad_y) and convert f32 -> f16, for
// the implicit-GEMM planar conv path.
static __global__ void makepad_cuda_pad_planar_f32_to_f16_kernel(
        const float * __restrict__ src,
        __half * __restrict__ dst,
        uint32_t width,
        uint32_t height,
        uint32_t channels,
        uint32_t pad_x,
        uint32_t pad_y) {
    const uint32_t out_width = width + 2 * pad_x;
    const uint32_t out_height = height + 2 * pad_y;
    const size_t total = static_cast<size_t>(out_width) * out_height * channels;
    const size_t idx = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    if (idx >= total) {
        return;
    }
    const uint32_t x = static_cast<uint32_t>(idx % out_width);
    const size_t rest = idx / out_width;
    const uint32_t y = static_cast<uint32_t>(rest % out_height);
    const uint32_t c = static_cast<uint32_t>(rest / out_height);
    float value = 0.0f;
    if (x >= pad_x && x < pad_x + width && y >= pad_y && y < pad_y + height) {
        const size_t src_idx = static_cast<size_t>(x - pad_x)
            + static_cast<size_t>(width) * ((y - pad_y) + static_cast<size_t>(height) * c);
        value = src[src_idx];
    }
    dst[idx] = __float2half(value);
}

extern "C" cudaError_t makepad_cuda_pad_planar_f32_to_f16(
        const float * src,
        uint16_t * dst,
        uint32_t width,
        uint32_t height,
        uint32_t channels,
        uint32_t pad_x,
        uint32_t pad_y,
        cudaStream_t stream) {
    const size_t total = static_cast<size_t>(width + 2 * pad_x)
        * (height + 2 * pad_y) * channels;
    if (total == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<unsigned int>((total + block.x - 1) / block.x), 1, 1);
    makepad_cuda_pad_planar_f32_to_f16_kernel<<<grid, block, 0, stream>>>(
        src, reinterpret_cast<__half *>(dst), width, height, channels, pad_x, pad_y);
    return cudaGetLastError();
}

// Extract the valid interior of a padded-plane conv accumulator and add the
// per-channel bias: out[oc][y*W+x] = acc[oc*padded_plane + y*padded_width + x]
// + bias[oc]. The accumulator rows beyond the interior are discarded.
static __global__ void makepad_cuda_conv_extract_bias_f32_kernel(
        const float * __restrict__ acc,
        const float * __restrict__ bias,
        float * __restrict__ out,
        uint32_t width,
        uint32_t height,
        uint32_t padded_width,
        uint32_t padded_plane,
        uint32_t out_channels) {
    const size_t total = static_cast<size_t>(width) * height * out_channels;
    const size_t idx = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    if (idx >= total) {
        return;
    }
    const uint32_t x = static_cast<uint32_t>(idx % width);
    const size_t rest = idx / width;
    const uint32_t y = static_cast<uint32_t>(rest % height);
    const uint32_t oc = static_cast<uint32_t>(rest / height);
    const size_t src = static_cast<size_t>(oc) * padded_plane
        + static_cast<size_t>(y) * padded_width + x;
    out[idx] = acc[src] + bias[oc];
}

extern "C" cudaError_t makepad_cuda_conv_extract_bias_f32(
        const float * acc,
        const float * bias,
        float * out,
        uint32_t width,
        uint32_t height,
        uint32_t padded_width,
        uint32_t padded_plane,
        uint32_t out_channels,
        cudaStream_t stream) {
    const size_t total = static_cast<size_t>(width) * height * out_channels;
    if (total == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<unsigned int>((total + block.x - 1) / block.x), 1, 1);
    makepad_cuda_conv_extract_bias_f32_kernel<<<grid, block, 0, stream>>>(
        acc, bias, out, width, height, padded_width, padded_plane, out_channels);
    return cudaGetLastError();
}

// out[c][p] += vec[c] for planar data (per-plane bias add).
static __global__ void makepad_cuda_add_planes_vec_f32_kernel(
        float * __restrict__ data,
        const float * __restrict__ vec,
        uint32_t plane,
        uint32_t channels) {
    const size_t total = static_cast<size_t>(plane) * channels;
    const size_t idx = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    if (idx >= total) {
        return;
    }
    data[idx] += vec[idx / plane];
}

extern "C" cudaError_t makepad_cuda_add_planes_vec_f32(
        float * data,
        const float * vec,
        uint32_t plane,
        uint32_t channels,
        cudaStream_t stream) {
    const size_t total = static_cast<size_t>(plane) * channels;
    if (total == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<unsigned int>((total + block.x - 1) / block.x), 1, 1);
    makepad_cuda_add_planes_vec_f32_kernel<<<grid, block, 0, stream>>>(
        data, vec, plane, channels);
    return cudaGetLastError();
}

// ---------------------------------------------------------------------------
// Fused flash attention (non-causal, head_dim 128 only): flash-attention-2
// style online-softmax tiling — the [heads x seq x seq] score tensor is never
// materialized, replacing the cublas composite recipe whose 3+GB/call of
// score/prob traffic was the floor at seq 4608. q/k/v are token-major
// [token][head][dim] PRE-CONVERTED to f16 (row stride `hidden`), out is f32.
// Tiles are staged in shared via cp.async and multiplied on tensor cores
// (wmma) with f32 accumulators; the softmax itself is entirely f32 — the
// same numeric recipe as the composite path (f16 gemm inputs, f32
// softmax/accumulation).
//
// Block layout: one block per (64-row query tile, head); 8 warps.
//   - S = Q_tile(64x128) K_tile^T(128x64) via wmma into an f32 shared tile.
//   - Online softmax by row-owning thread quads (4 threads per row): running
//     max m / sum l / output rescale in registers, P written to shared f16.
//   - O += P(64x64) V_tile(64x128) via wmma, in two 64-column halves through
//     the same f32 scratch tile, accumulated into per-thread registers.
// Rows/cols past `seq` load as zeros and are masked to -inf in the softmax.
constexpr int FA_BR = 64;         // query rows per block
constexpr int FA_BC = 64;         // key rows per tile iteration
constexpr int FA_D = 128;         // head dim (the only supported value)
constexpr int FA_LDQ = FA_D + 8;  // f16 q/k/v tile leading dim
constexpr int FA_LDS = FA_BC + 8; // f32 score / f16 prob tile leading dim
constexpr int FA_THREADS = 256;   // 8 warps
constexpr size_t FA_SMEM_Q = static_cast<size_t>(FA_BR) * FA_LDQ * sizeof(__half);
// K tiles are double-buffered (cp.async prefetch of tile i+1 overlaps all of
// iteration i); V is single-buffered but its cp.async is issued at the top of
// the iteration and only awaited before the PV gemms, so it overlaps the
// QK gemm + softmax. The inputs are pre-converted f16 so cp.async can copy
// raw bytes (it cannot convert).
constexpr size_t FA_SMEM_K = 2 * static_cast<size_t>(FA_BC) * FA_LDQ * sizeof(__half);
constexpr size_t FA_SMEM_V = static_cast<size_t>(FA_BC) * FA_LDQ * sizeof(__half);
constexpr size_t FA_SMEM_S = static_cast<size_t>(FA_BR) * FA_LDS * sizeof(float);
constexpr size_t FA_SMEM_P = static_cast<size_t>(FA_BR) * FA_LDS * sizeof(__half);
constexpr size_t FA_SMEM_TOTAL = FA_SMEM_Q + FA_SMEM_K + FA_SMEM_V + FA_SMEM_S + FA_SMEM_P;

// One cp.async 16-byte copy; src_bytes < 16 zero-fills the remainder (used to
// zero rows past `seq` without a branch on the destination side).
static __device__ __forceinline__ void makepad_cuda_fa_cp_async16(
        void * dst_shared,
        const void * src_global,
        int src_bytes) {
    const unsigned dst = static_cast<unsigned>(__cvta_generic_to_shared(dst_shared));
    asm volatile(
        "cp.async.cg.shared.global [%0], [%1], 16, %2;\n"
        :
        : "r"(dst), "l"(src_global), "r"(src_bytes));
}

// Issue the cp.async copies for one 64x128 f16 tile: 4 threads per row, 64
// bytes (4 x 16B) each. Rows past `seq` become zeros via src_bytes = 0.
static __device__ __forceinline__ void makepad_cuda_fa_tile_async(
        const __half * __restrict__ src,
        __half * __restrict__ dst,
        uint32_t row0,
        uint32_t seq,
        uint32_t hidden,
        uint32_t col0) {
    const int r = threadIdx.x >> 2;
    const int quarter = threadIdx.x & 3;
    const uint32_t row = row0 + r;
    const int src_bytes = row < seq ? 16 : 0;
    // Clamp the OOB source pointer back in range: with src_bytes = 0 nothing
    // is read, but the address must still be safe to form.
    const size_t src_row = row < seq ? row : 0;
    const __half * in = src + src_row * hidden + col0 + quarter * 32;
    __half * out = dst + r * FA_LDQ + quarter * 32;
    #pragma unroll
    for (int i = 0; i < 4; i++) {
        makepad_cuda_fa_cp_async16(out + i * 8, in + i * 8, src_bytes);
    }
}

// Load one 64x128 tile (rows row0..row0+64 of a token-major tensor, columns
// col0..col0+128) into a shared f16 tile; rows past `seq` load as zeros.
// 4 threads per row, 32 consecutive floats each (vectorized float4 reads).
static __device__ __forceinline__ void makepad_cuda_fa_cp_commit() {
    asm volatile("cp.async.commit_group;\n");
}

template <int PENDING>
static __device__ __forceinline__ void makepad_cuda_fa_cp_wait() {
    asm volatile("cp.async.wait_group %0;\n" : : "n"(PENDING));
}

// Synchronous f16 tile load (used once for the Q tile): 4 threads per row,
// 32 halves (4 x uint4) each; rows past `seq` load as zeros.
static __device__ __forceinline__ void makepad_cuda_fa_load_tile(
        const __half * __restrict__ src,
        __half * __restrict__ dst,
        uint32_t row0,
        uint32_t seq,
        uint32_t hidden,
        uint32_t col0) {
    const int r = threadIdx.x >> 2;
    const int quarter = threadIdx.x & 3;
    const uint32_t row = row0 + r;
    __half * out = dst + r * FA_LDQ + quarter * 32;
    if (row < seq) {
        const __half * in = src + static_cast<size_t>(row) * hidden + col0 + quarter * 32;
        #pragma unroll
        for (int i = 0; i < 4; i++) {
            *reinterpret_cast<uint4 *>(out + i * 8) =
                *reinterpret_cast<const uint4 *>(in + i * 8);
        }
    } else {
        const __half zero = __float2half_rn(0.0f);
        #pragma unroll
        for (int i = 0; i < 32; i++) {
            out[i] = zero;
        }
    }
}

static __global__ void makepad_cuda_flash_attention_f32_kernel(
        const __half * __restrict__ q,
        const __half * __restrict__ k,
        const __half * __restrict__ v,
        float * __restrict__ out,
        uint32_t seq,
        uint32_t hidden,
        float scale) {
    extern __shared__ __align__(16) char fa_smem[];
    __half * q_sh = reinterpret_cast<__half *>(fa_smem);
    __half * k_ring = reinterpret_cast<__half *>(fa_smem + FA_SMEM_Q);
    __half * v_sh = reinterpret_cast<__half *>(fa_smem + FA_SMEM_Q + FA_SMEM_K);
    float * s_sh = reinterpret_cast<float *>(fa_smem + FA_SMEM_Q + FA_SMEM_K + FA_SMEM_V);
    __half * p_sh = reinterpret_cast<__half *>(
        fa_smem + FA_SMEM_Q + FA_SMEM_K + FA_SMEM_V + FA_SMEM_S);
    constexpr int FA_K_STAGE = FA_BC * FA_LDQ; // halves per K ring stage

    const uint32_t q0 = blockIdx.x * FA_BR;
    const uint32_t col0 = blockIdx.y * FA_D;
    if (q0 >= seq) {
        return;
    }

    // wmma tile assignment: 8 warps in a 4x2 grid over a 64-row tile.
    const int warp = threadIdx.x >> 5;
    const int warp_row = (warp >> 1) * 16;
    const int warp_col = (warp & 1) * 32;

    // Softmax/output row ownership: 4 threads per row; each owns 16 score
    // columns and 32 output columns.
    const int own_row = threadIdx.x >> 2;
    const int own_sub = threadIdx.x & 3;

    float o_acc[32];
    #pragma unroll
    for (int i = 0; i < 32; i++) {
        o_acc[i] = 0.0f;
    }
    float m_row = -CUDART_INF_F;
    float l_row = 0.0f;

    const uint32_t tiles = (seq + FA_BC - 1) / FA_BC;
    // Prologue: K(0) prefetch in flight while the Q tile loads synchronously.
    makepad_cuda_fa_tile_async(k, k_ring, 0, seq, hidden, col0);
    makepad_cuda_fa_cp_commit();
    makepad_cuda_fa_load_tile(q, q_sh, q0, seq, hidden, col0);
    makepad_cuda_fa_cp_wait<0>();
    __syncthreads();

    for (uint32_t tile = 0; tile < tiles; tile++) {
        const uint32_t k0 = tile * FA_BC;
        const __half * k_sh = k_ring + (tile & 1) * FA_K_STAGE;
        // V(tile) group: awaited just before the PV gemms, overlapping the
        // QK gemm and softmax below.
        makepad_cuda_fa_tile_async(v, v_sh, k0, seq, hidden, col0);
        makepad_cuda_fa_cp_commit();
        // K(tile+1) group: awaited at the end of this iteration.
        if (tile + 1 < tiles) {
            makepad_cuda_fa_tile_async(
                k, k_ring + ((tile + 1) & 1) * FA_K_STAGE,
                k0 + FA_BC, seq, hidden, col0);
        }
        makepad_cuda_fa_cp_commit();

        // S = Q K^T for this tile (f32 accumulators -> s_sh).
        {
            nvcuda::wmma::fragment<nvcuda::wmma::matrix_a, 16, 16, 16, __half,
                nvcuda::wmma::row_major> a_frag;
            nvcuda::wmma::fragment<nvcuda::wmma::matrix_b, 16, 16, 16, __half,
                nvcuda::wmma::col_major> b_frag;
            nvcuda::wmma::fragment<nvcuda::wmma::accumulator, 16, 16, 16, float> c_frag[2];
            nvcuda::wmma::fill_fragment(c_frag[0], 0.0f);
            nvcuda::wmma::fill_fragment(c_frag[1], 0.0f);
            #pragma unroll
            for (int kk = 0; kk < FA_D / 16; kk++) {
                nvcuda::wmma::load_matrix_sync(
                    a_frag, q_sh + warp_row * FA_LDQ + kk * 16, FA_LDQ);
                #pragma unroll
                for (int n = 0; n < 2; n++) {
                    // K^T as a col_major view of the row-major K tile.
                    nvcuda::wmma::load_matrix_sync(
                        b_frag, k_sh + (warp_col + n * 16) * FA_LDQ + kk * 16, FA_LDQ);
                    nvcuda::wmma::mma_sync(c_frag[n], a_frag, b_frag, c_frag[n]);
                }
            }
            #pragma unroll
            for (int n = 0; n < 2; n++) {
                nvcuda::wmma::store_matrix_sync(
                    s_sh + warp_row * FA_LDS + warp_col + n * 16,
                    c_frag[n], FA_LDS, nvcuda::wmma::mem_row_major);
            }
        }
        __syncthreads();

        // Online softmax on this thread's 16-column row segment.
        {
            const uint32_t remaining = seq - k0;
            const uint32_t valid = remaining < FA_BC ? remaining : FA_BC;
            const float * s_row = s_sh + own_row * FA_LDS;
            float seg[16];
            float tile_max = -CUDART_INF_F;
            #pragma unroll
            for (int i = 0; i < 16; i++) {
                const uint32_t c = own_sub * 16 + i;
                const float value = c < valid ? s_row[c] * scale : -CUDART_INF_F;
                seg[i] = value;
                tile_max = value > tile_max ? value : tile_max;
            }
            tile_max = fmaxf(tile_max, __shfl_xor_sync(0xffffffffu, tile_max, 1));
            tile_max = fmaxf(tile_max, __shfl_xor_sync(0xffffffffu, tile_max, 2));
            const float m_new = m_row > tile_max ? m_row : tile_max;
            const float rescale = expf(m_row - m_new);
            float sum = 0.0f;
            __half * p_row = p_sh + own_row * FA_LDS;
            #pragma unroll
            for (int i = 0; i < 16; i++) {
                const uint32_t c = own_sub * 16 + i;
                const float p = c < valid ? expf(seg[i] - m_new) : 0.0f;
                p_row[c] = __float2half_rn(p);
                sum += p;
            }
            sum += __shfl_xor_sync(0xffffffffu, sum, 1);
            sum += __shfl_xor_sync(0xffffffffu, sum, 2);
            l_row = l_row * rescale + sum;
            m_row = m_new;
            #pragma unroll
            for (int i = 0; i < 32; i++) {
                o_acc[i] *= rescale;
            }
        }
        __syncthreads();

        // The V(tile) prefetch must land before the PV gemms; K(tile+1) may
        // still be in flight (one outstanding group).
        makepad_cuda_fa_cp_wait<1>();
        __syncthreads();

        // O += P V, in two 64-column halves through the s_sh scratch tile.
        #pragma unroll
        for (int half = 0; half < 2; half++) {
            {
                nvcuda::wmma::fragment<nvcuda::wmma::matrix_a, 16, 16, 16, __half,
                    nvcuda::wmma::row_major> a_frag;
                nvcuda::wmma::fragment<nvcuda::wmma::matrix_b, 16, 16, 16, __half,
                    nvcuda::wmma::row_major> b_frag;
                nvcuda::wmma::fragment<nvcuda::wmma::accumulator, 16, 16, 16, float> c_frag[2];
                nvcuda::wmma::fill_fragment(c_frag[0], 0.0f);
                nvcuda::wmma::fill_fragment(c_frag[1], 0.0f);
                #pragma unroll
                for (int kk = 0; kk < FA_BC / 16; kk++) {
                    nvcuda::wmma::load_matrix_sync(
                        a_frag, p_sh + warp_row * FA_LDS + kk * 16, FA_LDS);
                    #pragma unroll
                    for (int n = 0; n < 2; n++) {
                        nvcuda::wmma::load_matrix_sync(
                            b_frag,
                            v_sh + kk * 16 * FA_LDQ + half * 64 + warp_col + n * 16,
                            FA_LDQ);
                        nvcuda::wmma::mma_sync(c_frag[n], a_frag, b_frag, c_frag[n]);
                    }
                }
                #pragma unroll
                for (int n = 0; n < 2; n++) {
                    nvcuda::wmma::store_matrix_sync(
                        s_sh + warp_row * FA_LDS + warp_col + n * 16,
                        c_frag[n], FA_LDS, nvcuda::wmma::mem_row_major);
                }
            }
            __syncthreads();
            if ((own_sub >> 1) == half) {
                const float * d_row = s_sh + own_row * FA_LDS + (own_sub & 1) * 32;
                #pragma unroll
                for (int i = 0; i < 32; i++) {
                    o_acc[i] += d_row[i];
                }
            }
            __syncthreads();
        }
        // K(tile+1) must be resident before the next iteration's QK gemm.
        makepad_cuda_fa_cp_wait<0>();
        __syncthreads();
    }

    // Normalize by the softmax denominator and write the owned column slice.
    const uint32_t out_row = q0 + own_row;
    if (out_row < seq) {
        const float inv = l_row > 0.0f ? 1.0f / l_row : 0.0f;
        float * dst = out + static_cast<size_t>(out_row) * hidden + col0 + own_sub * 32;
        #pragma unroll
        for (int i = 0; i < 32; i += 4) {
            float4 f;
            f.x = o_acc[i] * inv;
            f.y = o_acc[i + 1] * inv;
            f.z = o_acc[i + 2] * inv;
            f.w = o_acc[i + 3] * inv;
            *reinterpret_cast<float4 *>(dst + i) = f;
        }
    }
}

extern "C" cudaError_t makepad_cuda_flash_attention_f32(
        const uint16_t * q,
        const uint16_t * k,
        const uint16_t * v,
        float * out,
        uint32_t seq,
        uint32_t head_count,
        uint32_t hidden,
        float scale,
        cudaStream_t stream) {
    if (seq == 0 || head_count == 0) {
        return cudaSuccess;
    }
    if (hidden != head_count * FA_D) {
        return cudaErrorInvalidValue;
    }
    // The ~78KB shared tile set needs the opt-in dynamic shared memory limit.
    static bool fa_smem_configured = false;
    if (!fa_smem_configured) {
        const cudaError_t err = cudaFuncSetAttribute(
            makepad_cuda_flash_attention_f32_kernel,
            cudaFuncAttributeMaxDynamicSharedMemorySize,
            static_cast<int>(FA_SMEM_TOTAL));
        if (err != cudaSuccess) {
            return err;
        }
        fa_smem_configured = true;
    }
    const dim3 block(FA_THREADS, 1, 1);
    const dim3 grid((seq + FA_BR - 1) / FA_BR, head_count, 1);
    makepad_cuda_flash_attention_f32_kernel
        <<<grid, block, FA_SMEM_TOTAL, stream>>>(
            reinterpret_cast<const __half *>(q),
            reinterpret_cast<const __half *>(k),
            reinterpret_cast<const __half *>(v),
            out, seq, hidden, scale);
    return cudaGetLastError();
}

// ---------------------------------------------------------------------------
// Register-level FA2 flash attention (raw mma.sync path).
//
// The wmma kernel above round-trips S and the PV product through shared
// memory (~7 __syncthreads per 64-key tile). This kernel keeps the FA2 state
// in registers instead:
//   - each warp OWNS 16 query rows for the whole KV loop (no cross-warp
//     reduction): Q lives in A-fragments, S/P/O in mma fragments, the online
//     softmax (m, l, rescale) on the fragment lanes;
//   - P is repacked from S accumulator fragments straight into A-operand
//     fragments (the m16n8k16 C->A layout identity) - no shared S/P tiles;
//   - shared memory holds only the double-buffered K and V tile rings
//     (cp.async, one commit group per tile) -> 2 __syncthreads per tile.
// Numerics are the same recipe as the wmma kernel: f16 gemm inputs, f32
// softmax and accumulators, expf, final 1/l normalize.
//
// Block = (128 query rows, 1 head): 8 warps x m16. The 136-half row pitch
// (FA_LDQ) makes every ldmatrix 8-row read conflict-free (row stride 272B =
// 68 words = a 4-bank rotation per row).
constexpr int FA2_BR = 128;      // query rows per block (8 warps x 16 rows)
constexpr int FA2_BC = 64;       // key rows per tile iteration
constexpr int FA2_THREADS = 256;
constexpr int FA2_STAGE = FA2_BC * FA_LDQ; // halves per K/V ring stage
constexpr size_t FA2_SMEM_TOTAL =
    4 * static_cast<size_t>(FA2_STAGE) * sizeof(__half); // K ring x2 + V ring x2

static __device__ __forceinline__ void makepad_cuda_fa2_mma(
        float c[4], const uint32_t a[4], const uint32_t b0, const uint32_t b1) {
    asm volatile(
        "mma.sync.aligned.m16n8k16.row.col.f32.f16.f16.f32 "
        "{%0,%1,%2,%3}, {%4,%5,%6,%7}, {%8,%9}, {%0,%1,%2,%3};\n"
        : "+f"(c[0]), "+f"(c[1]), "+f"(c[2]), "+f"(c[3])
        : "r"(a[0]), "r"(a[1]), "r"(a[2]), "r"(a[3]), "r"(b0), "r"(b1));
}

static __device__ __forceinline__ void makepad_cuda_fa2_mma_bf16(
        float c[4], const uint32_t a[4], const uint32_t b0, const uint32_t b1) {
    asm volatile(
        "mma.sync.aligned.m16n8k16.row.col.f32.bf16.bf16.f32 "
        "{%0,%1,%2,%3}, {%4,%5,%6,%7}, {%8,%9}, {%0,%1,%2,%3};\n"
        : "+f"(c[0]), "+f"(c[1]), "+f"(c[2]), "+f"(c[3])
        : "r"(a[0]), "r"(a[1]), "r"(a[2]), "r"(a[3]), "r"(b0), "r"(b1));
}

static __device__ __forceinline__ void makepad_cuda_fa2_ldmatrix_x4(
        uint32_t r[4], const __half * addr) {
    const unsigned p = static_cast<unsigned>(__cvta_generic_to_shared(addr));
    asm volatile(
        "ldmatrix.sync.aligned.m8n8.x4.shared.b16 {%0,%1,%2,%3}, [%4];\n"
        : "=r"(r[0]), "=r"(r[1]), "=r"(r[2]), "=r"(r[3])
        : "r"(p));
}

static __device__ __forceinline__ void makepad_cuda_fa2_ldmatrix_x4_trans(
        uint32_t r[4], const __half * addr) {
    const unsigned p = static_cast<unsigned>(__cvta_generic_to_shared(addr));
    asm volatile(
        "ldmatrix.sync.aligned.m8n8.x4.trans.shared.b16 {%0,%1,%2,%3}, [%4];\n"
        : "=r"(r[0]), "=r"(r[1]), "=r"(r[2]), "=r"(r[3])
        : "r"(p));
}

static __device__ __forceinline__ uint32_t makepad_cuda_fa2_pack(
        float a, float b) {
    const __half2 h = __floats2half2_rn(a, b);
    return *reinterpret_cast<const uint32_t *>(&h);
}

static __device__ __forceinline__ uint32_t makepad_cuda_fa2_pack_bf16(
        float a, float b) {
    const __nv_bfloat162 h = __floats2bfloat162_rn(a, b);
    return *reinterpret_cast<const uint32_t *>(&h);
}

template<bool Causal, bool UseBf16>
static __global__ void makepad_cuda_flash_attention2_f32_kernel(
        const __half * __restrict__ q,
        const __half * __restrict__ k,
        const __half * __restrict__ v,
        float * __restrict__ out,
        uint32_t seq,
        uint32_t kv_len,
        uint32_t hidden,
        float scale,
        int32_t window) {
    extern __shared__ __align__(16) char fa2_smem[];
    __half * k_ring = reinterpret_cast<__half *>(fa2_smem);
    __half * v_ring = k_ring + 2 * FA2_STAGE;

    const uint32_t q0 = blockIdx.x * FA2_BR;
    const uint32_t col0 = blockIdx.y * FA_D;
    if (q0 >= seq) {
        return;
    }

    const int warp = threadIdx.x >> 5;
    const int lane = threadIdx.x & 31;
    const int lane_row = lane >> 2;    // fragment row group (0..7)
    const int lane_quad = lane & 3;    // fragment column pair index
    // FA2 fragment owns two query rows: lane_row and lane_row+8.
    const uint32_t q_lo = q0 + warp * 16 + lane_row;
    const uint32_t q_hi = q_lo + 8;

    // --- Prologue: stage Q through the K ring, then park it in registers. --
    // Rows q0..q0+64 -> stage 0, q0+64..q0+128 -> stage 1 (zero-filled past
    // seq). Warp w reads its 16 rows (16w..16w+16) from stage (w >= 4).
    makepad_cuda_fa_tile_async(q, k_ring, q0, seq, hidden, col0);
    makepad_cuda_fa_tile_async(q, k_ring + FA2_STAGE, q0 + FA2_BC, seq, hidden, col0);
    makepad_cuda_fa_cp_commit();
    makepad_cuda_fa_cp_wait<0>();
    __syncthreads();

    uint32_t q_frag[8][4]; // 16 x 128 as 8 k-chunks of m16k16
    {
        const int row_in_stage = (warp * 16) & (FA2_BC - 1);
        const __half * q_sh = k_ring + (warp >= 4 ? FA2_STAGE : 0)
            + row_in_stage * FA_LDQ;
        #pragma unroll
        for (int kk = 0; kk < 8; kk++) {
            // lanes 0..15: rows, halves 0-7; lanes 16..31: same rows, halves 8-15.
            const __half * addr = q_sh + (lane & 15) * FA_LDQ + kk * 16 + (lane >> 4) * 8;
            makepad_cuda_fa2_ldmatrix_x4(q_frag[kk], addr);
        }
    }
    __syncthreads(); // all warps done with the staging area

    float o_acc[16][4]; // 16 x 128 output as 16 n8 accumulator fragments
    #pragma unroll
    for (int j = 0; j < 16; j++) {
        o_acc[j][0] = 0.0f;
        o_acc[j][1] = 0.0f;
        o_acc[j][2] = 0.0f;
        o_acc[j][3] = 0.0f;
    }
    float m_lo = -CUDART_INF_F, m_hi = -CUDART_INF_F;
    float l_lo = 0.0f, l_hi = 0.0f;

    const uint32_t tiles = (kv_len + FA2_BC - 1) / FA2_BC;
    // G(0): K/V tile 0 into ring stage 0.
    makepad_cuda_fa_tile_async(k, k_ring, 0, kv_len, hidden, col0);
    makepad_cuda_fa_tile_async(v, v_ring, 0, kv_len, hidden, col0);
    makepad_cuda_fa_cp_commit();

    for (uint32_t tile = 0; tile < tiles; tile++) {
        const uint32_t k0 = tile * FA2_BC;
        const int stage = tile & 1;
        // G(tile+1): prefetch overlaps all of this iteration's compute. Its
        // target stage held tile-1, whose readers finished at the bottom
        // sync of the previous iteration.
        if (tile + 1 < tiles) {
            makepad_cuda_fa_tile_async(
                k, k_ring + (stage ^ 1) * FA2_STAGE, k0 + FA2_BC, kv_len, hidden, col0);
            makepad_cuda_fa_tile_async(
                v, v_ring + (stage ^ 1) * FA2_STAGE, k0 + FA2_BC, kv_len, hidden, col0);
            makepad_cuda_fa_cp_commit();
            makepad_cuda_fa_cp_wait<1>(); // G(tile) landed, G(tile+1) in flight
        } else {
            makepad_cuda_fa_cp_wait<0>();
        }
        __syncthreads();

        const __half * k_sh = k_ring + stage * FA2_STAGE;
        const __half * v_sh = v_ring + stage * FA2_STAGE;

        // S = Q K^T: 8 n8 fragments per warp, f32 accumulators.
        float s[8][4];
        #pragma unroll
        for (int j = 0; j < 8; j++) {
            s[j][0] = 0.0f;
            s[j][1] = 0.0f;
            s[j][2] = 0.0f;
            s[j][3] = 0.0f;
        }
        #pragma unroll
        for (int j = 0; j < 8; j += 2) {
            #pragma unroll
            for (int kk = 0; kk < 8; kk++) {
                // x4 non-trans on K rows = B fragments for n-tiles j, j+1:
                // {b0,b1} = keys 8j.., {b2,b3} = keys 8(j+1).. (16 dims each).
                const int sel = lane >> 3;
                const __half * addr = k_sh
                    + (8 * (j + (sel >> 1)) + (lane & 7)) * FA_LDQ
                    + kk * 16 + (sel & 1) * 8;
                uint32_t b[4];
                makepad_cuda_fa2_ldmatrix_x4(b, addr);
                if constexpr (UseBf16) {
                    makepad_cuda_fa2_mma_bf16(s[j], q_frag[kk], b[0], b[1]);
                    makepad_cuda_fa2_mma_bf16(s[j + 1], q_frag[kk], b[2], b[3]);
                } else {
                    makepad_cuda_fa2_mma(s[j], q_frag[kk], b[0], b[1]);
                    makepad_cuda_fa2_mma(s[j + 1], q_frag[kk], b[2], b[3]);
                }
            }
        }

        // Online softmax on the fragment lanes. Thread rows: lane_row (c0,c1)
        // and lane_row+8 (c2,c3); columns 8j + 2*lane_quad + {0,1}.
        const uint32_t remaining = kv_len - k0;
        const uint32_t valid = remaining < FA2_BC ? remaining : FA2_BC;
        float tmax_lo = -CUDART_INF_F, tmax_hi = -CUDART_INF_F;
        #pragma unroll
        for (int j = 0; j < 8; j++) {
            const uint32_t c = 8 * j + 2 * lane_quad;
            const uint32_t key0 = k0 + c;
            const uint32_t key1 = key0 + 1;
            bool a0 = c < valid;
            bool a1 = c + 1 < valid;
            bool a2 = c < valid;
            bool a3 = c + 1 < valid;
            if constexpr (Causal) {
                a0 = a0 && key0 <= q_lo;
                a1 = a1 && key1 <= q_lo;
                a2 = a2 && key0 <= q_hi;
                a3 = a3 && key1 <= q_hi;
            }
            if (window > 0) {
                const int32_t w = window;
                a0 = a0 && (int32_t)key0 - (int32_t)q_lo <= w && (int32_t)q_lo - (int32_t)key0 <= w;
                a1 = a1 && (int32_t)key1 - (int32_t)q_lo <= w && (int32_t)q_lo - (int32_t)key1 <= w;
                a2 = a2 && (int32_t)key0 - (int32_t)q_hi <= w && (int32_t)q_hi - (int32_t)key0 <= w;
                a3 = a3 && (int32_t)key1 - (int32_t)q_hi <= w && (int32_t)q_hi - (int32_t)key1 <= w;
            }
            s[j][0] = a0 ? s[j][0] * scale : -CUDART_INF_F;
            s[j][1] = a1 ? s[j][1] * scale : -CUDART_INF_F;
            s[j][2] = a2 ? s[j][2] * scale : -CUDART_INF_F;
            s[j][3] = a3 ? s[j][3] * scale : -CUDART_INF_F;
            tmax_lo = fmaxf(tmax_lo, fmaxf(s[j][0], s[j][1]));
            tmax_hi = fmaxf(tmax_hi, fmaxf(s[j][2], s[j][3]));
        }
        tmax_lo = fmaxf(tmax_lo, __shfl_xor_sync(0xffffffffu, tmax_lo, 1));
        tmax_lo = fmaxf(tmax_lo, __shfl_xor_sync(0xffffffffu, tmax_lo, 2));
        tmax_hi = fmaxf(tmax_hi, __shfl_xor_sync(0xffffffffu, tmax_hi, 1));
        tmax_hi = fmaxf(tmax_hi, __shfl_xor_sync(0xffffffffu, tmax_hi, 2));
        const float m_new_lo = m_lo > tmax_lo ? m_lo : tmax_lo;
        const float m_new_hi = m_hi > tmax_hi ? m_hi : tmax_hi;
        const float rescale_lo = expf(m_lo - m_new_lo);
        const float rescale_hi = expf(m_hi - m_new_hi);
        float sum_lo = 0.0f, sum_hi = 0.0f;
        uint32_t p_frag[4][4]; // P as 4 k16 A-operand fragments
        #pragma unroll
        for (int kk2 = 0; kk2 < 4; kk2++) {
            const int j0 = 2 * kk2;
            const float p00 = expf(s[j0][0] - m_new_lo);
            const float p01 = expf(s[j0][1] - m_new_lo);
            const float p02 = expf(s[j0][2] - m_new_hi);
            const float p03 = expf(s[j0][3] - m_new_hi);
            const float p10 = expf(s[j0 + 1][0] - m_new_lo);
            const float p11 = expf(s[j0 + 1][1] - m_new_lo);
            const float p12 = expf(s[j0 + 1][2] - m_new_hi);
            const float p13 = expf(s[j0 + 1][3] - m_new_hi);
            sum_lo += p00 + p01 + p10 + p11;
            sum_hi += p02 + p03 + p12 + p13;
            // C->A fragment identity: a0/a1 = rows (lo,hi) x keys 2q..2q+1 of
            // tile j0; a2/a3 = the same rows in tile j0+1 (keys +8).
            if constexpr (UseBf16) {
                p_frag[kk2][0] = makepad_cuda_fa2_pack_bf16(p00, p01);
                p_frag[kk2][1] = makepad_cuda_fa2_pack_bf16(p02, p03);
                p_frag[kk2][2] = makepad_cuda_fa2_pack_bf16(p10, p11);
                p_frag[kk2][3] = makepad_cuda_fa2_pack_bf16(p12, p13);
            } else {
                p_frag[kk2][0] = makepad_cuda_fa2_pack(p00, p01);
                p_frag[kk2][1] = makepad_cuda_fa2_pack(p02, p03);
                p_frag[kk2][2] = makepad_cuda_fa2_pack(p10, p11);
                p_frag[kk2][3] = makepad_cuda_fa2_pack(p12, p13);
            }
        }
        sum_lo += __shfl_xor_sync(0xffffffffu, sum_lo, 1);
        sum_lo += __shfl_xor_sync(0xffffffffu, sum_lo, 2);
        sum_hi += __shfl_xor_sync(0xffffffffu, sum_hi, 1);
        sum_hi += __shfl_xor_sync(0xffffffffu, sum_hi, 2);
        l_lo = l_lo * rescale_lo + sum_lo;
        l_hi = l_hi * rescale_hi + sum_hi;
        m_lo = m_new_lo;
        m_hi = m_new_hi;
        #pragma unroll
        for (int j = 0; j < 16; j++) {
            o_acc[j][0] *= rescale_lo;
            o_acc[j][1] *= rescale_lo;
            o_acc[j][2] *= rescale_hi;
            o_acc[j][3] *= rescale_hi;
        }

        // O += P V: 16 n8 fragments per warp, V via x4 trans loads.
        #pragma unroll
        for (int jj = 0; jj < 16; jj += 2) {
            #pragma unroll
            for (int kk2 = 0; kk2 < 4; kk2++) {
                // {b0,b1} = dims 8jj.., {b2,b3} = dims 8(jj+1).. (16 keys each).
                const int sel = lane >> 3;
                const __half * addr = v_sh
                    + (16 * kk2 + 8 * (sel & 1) + (lane & 7)) * FA_LDQ
                    + 8 * (jj + (sel >> 1));
                uint32_t b[4];
                makepad_cuda_fa2_ldmatrix_x4_trans(b, addr);
                if constexpr (UseBf16) {
                    makepad_cuda_fa2_mma_bf16(o_acc[jj], p_frag[kk2], b[0], b[1]);
                    makepad_cuda_fa2_mma_bf16(o_acc[jj + 1], p_frag[kk2], b[2], b[3]);
                } else {
                    makepad_cuda_fa2_mma(o_acc[jj], p_frag[kk2], b[0], b[1]);
                    makepad_cuda_fa2_mma(o_acc[jj + 1], p_frag[kk2], b[2], b[3]);
                }
            }
        }
        __syncthreads(); // all warps done with stage (tile&1) K/V reads
    }

    // Normalize and write the two owned rows (32B per quad per n-tile).
    const uint32_t row_lo = q0 + warp * 16 + lane_row;
    const uint32_t row_hi = row_lo + 8;
    const float inv_lo = l_lo > 0.0f ? 1.0f / l_lo : 0.0f;
    const float inv_hi = l_hi > 0.0f ? 1.0f / l_hi : 0.0f;
    if (row_lo < seq) {
        float * dst = out + static_cast<size_t>(row_lo) * hidden + col0 + 2 * lane_quad;
        #pragma unroll
        for (int j = 0; j < 16; j++) {
            float2 f;
            f.x = o_acc[j][0] * inv_lo;
            f.y = o_acc[j][1] * inv_lo;
            *reinterpret_cast<float2 *>(dst + 8 * j) = f;
        }
    }
    if (row_hi < seq) {
        float * dst = out + static_cast<size_t>(row_hi) * hidden + col0 + 2 * lane_quad;
        #pragma unroll
        for (int j = 0; j < 16; j++) {
            float2 f;
            f.x = o_acc[j][2] * inv_hi;
            f.y = o_acc[j][3] * inv_hi;
            *reinterpret_cast<float2 *>(dst + 8 * j) = f;
        }
    }
}

template<bool Causal, bool UseBf16>
static cudaError_t makepad_cuda_flash_attention2_launch(
        const uint16_t * q,
        const uint16_t * k,
        const uint16_t * v,
        float * out,
        uint32_t q_len,
        uint32_t kv_len,
        uint32_t head_count,
        uint32_t hidden,
        float scale,
        int32_t window,
        cudaStream_t stream) {
    if (q_len == 0 || head_count == 0) {
        return cudaSuccess;
    }
    if (kv_len == 0) {
        return cudaErrorInvalidValue;
    }
    if (hidden != head_count * FA_D) {
        return cudaErrorInvalidValue;
    }
    static bool smem_configured = false;
    if (!smem_configured) {
        const cudaError_t err = cudaFuncSetAttribute(
            makepad_cuda_flash_attention2_f32_kernel<Causal, UseBf16>,
            cudaFuncAttributeMaxDynamicSharedMemorySize,
            static_cast<int>(FA2_SMEM_TOTAL));
        if (err != cudaSuccess) {
            return err;
        }
        smem_configured = true;
    }
    const dim3 block(FA2_THREADS, 1, 1);
    const dim3 grid((q_len + FA2_BR - 1) / FA2_BR, head_count, 1);
    makepad_cuda_flash_attention2_f32_kernel<Causal, UseBf16>
        <<<grid, block, FA2_SMEM_TOTAL, stream>>>(
            reinterpret_cast<const __half *>(q),
            reinterpret_cast<const __half *>(k),
            reinterpret_cast<const __half *>(v),
            out, q_len, kv_len, hidden, scale, window);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_cuda_flash_attention2_f32(
        const uint16_t * q,
        const uint16_t * k,
        const uint16_t * v,
        float * out,
        uint32_t seq,
        uint32_t head_count,
        uint32_t hidden,
        float scale,
        cudaStream_t stream) {
    return makepad_cuda_flash_attention2_launch<false, false>(
        q, k, v, out, seq, seq, head_count, hidden, scale, 0, stream);
}

// Cross-attention flavor of the FA2 kernel: same body, kv length independent
// of the query length (TRELLIS image-cond cross-attn: q = tokens, kv = the
// fixed DINOv3 condition).
extern "C" cudaError_t makepad_cuda_flash_attention2_cross_f32(
        const uint16_t * q,
        const uint16_t * k,
        const uint16_t * v,
        float * out,
        uint32_t q_len,
        uint32_t kv_len,
        uint32_t head_count,
        uint32_t hidden,
        float scale,
        cudaStream_t stream) {
    return makepad_cuda_flash_attention2_launch<false, false>(
        q, k, v, out, q_len, kv_len, head_count, hidden, scale, 0, stream);
}

// Decode (q_len=1) / cross FA2 with bf16 tensor-core operands. For a single
// query at the end of a causal cache this is unmasked over the KV prefix.
extern "C" cudaError_t makepad_cuda_flash_attention2_cross_bf16(
        const uint16_t * q,
        const uint16_t * k,
        const uint16_t * v,
        float * out,
        uint32_t q_len,
        uint32_t kv_len,
        uint32_t head_count,
        uint32_t hidden,
        float scale,
        cudaStream_t stream) {
    return makepad_cuda_flash_attention2_launch<false, true>(
        q, k, v, out, q_len, kv_len, head_count, hidden, scale, 0, stream);
}

// Causal decoder-LM FA2. Same recipe as official SDPA/FlashAttention:
// online softmax, f16 or bf16 tensor-core QK/PV, f32 accumulators.
// Used by Music3 Qwen3 prefill (head_dim 128).
extern "C" cudaError_t makepad_cuda_flash_attention2_causal_f32(
        const uint16_t * q,
        const uint16_t * k,
        const uint16_t * v,
        float * out,
        uint32_t seq,
        uint32_t head_count,
        uint32_t hidden,
        float scale,
        cudaStream_t stream) {
    return makepad_cuda_flash_attention2_launch<true, false>(
        q, k, v, out, seq, seq, head_count, hidden, scale, 0, stream);
}

extern "C" cudaError_t makepad_cuda_flash_attention2_causal_bf16(
        const uint16_t * q,
        const uint16_t * k,
        const uint16_t * v,
        float * out,
        uint32_t seq,
        uint32_t head_count,
        uint32_t hidden,
        float scale,
        cudaStream_t stream) {
    return makepad_cuda_flash_attention2_launch<true, true>(
        q, k, v, out, seq, seq, head_count, hidden, scale, 0, stream);
}

extern "C" cudaError_t makepad_cuda_flash_attention2_sliding_bf16(
        const uint16_t * q,
        const uint16_t * k,
        const uint16_t * v,
        float * out,
        uint32_t seq,
        uint32_t head_count,
        uint32_t hidden,
        float scale,
        int32_t window,
        cudaStream_t stream) {
    return makepad_cuda_flash_attention2_launch<false, true>(
        q, k, v, out, seq, seq, head_count, hidden, scale, window, stream);
}

// ---------------------------------------------------------------------------
// BF16 flash attention for the SkinTokens family (head_dim 64).
//
// The released VAE and Michelangelo encoder execute PyTorch/FlashAttention
// with BF16 Q/K/V.  A materialized cuBLAS QK -> softmax -> PV recipe has a
// different reduction tree and, over the eight mesh blocks, its small error
// compounds into visibly different Qwen prefix tokens.  This kernel keeps
// BF16 tensor-core operands and an online f32 softmax/accumulator, while also
// removing the multi-gigabyte score slab required by 54k-point cross-attn.
// One implementation handles self and cross attention (`q_len != kv_len`).
constexpr int FAB_BR = 64;
constexpr int FAB_BC = 64;
constexpr int FAB_D = 64;
constexpr int FAB_LD = FAB_D + 8;
constexpr int FAB_LDS = FAB_BC + 8;
constexpr int FAB_THREADS = 256;
constexpr size_t FAB_SMEM_Q = static_cast<size_t>(FAB_BR) * FAB_LD * sizeof(__nv_bfloat16);
constexpr size_t FAB_SMEM_K = 2 * static_cast<size_t>(FAB_BC) * FAB_LD * sizeof(__nv_bfloat16);
constexpr size_t FAB_SMEM_V = static_cast<size_t>(FAB_BC) * FAB_LD * sizeof(__nv_bfloat16);
constexpr size_t FAB_SMEM_S = static_cast<size_t>(FAB_BR) * FAB_LDS * sizeof(float);
constexpr size_t FAB_SMEM_P = static_cast<size_t>(FAB_BR) * FAB_LDS * sizeof(__nv_bfloat16);
constexpr size_t FAB_SMEM_TOTAL = FAB_SMEM_Q + FAB_SMEM_K + FAB_SMEM_V + FAB_SMEM_S + FAB_SMEM_P;

static __device__ __forceinline__ void makepad_cuda_fab_tile_async(
        const __nv_bfloat16 * __restrict__ src,
        __nv_bfloat16 * __restrict__ dst,
        uint32_t row0,
        uint32_t rows,
        uint32_t hidden,
        uint32_t col0) {
    const int row_in_tile = threadIdx.x >> 2;
    const int quarter = threadIdx.x & 3;
    const uint32_t row = row0 + row_in_tile;
    const int src_bytes = row < rows ? 16 : 0;
    const size_t safe_row = row < rows ? row : 0;
    const __nv_bfloat16 * in = src + safe_row * hidden + col0 + quarter * 16;
    __nv_bfloat16 * out = dst + row_in_tile * FAB_LD + quarter * 16;
    #pragma unroll
    for (int i = 0; i < 2; i++) {
        makepad_cuda_fa_cp_async16(out + i * 8, in + i * 8, src_bytes);
    }
}

static __device__ __forceinline__ void makepad_cuda_fab_load_tile(
        const __nv_bfloat16 * __restrict__ src,
        __nv_bfloat16 * __restrict__ dst,
        uint32_t row0,
        uint32_t rows,
        uint32_t hidden,
        uint32_t col0) {
    const int row_in_tile = threadIdx.x >> 2;
    const int quarter = threadIdx.x & 3;
    const uint32_t row = row0 + row_in_tile;
    __nv_bfloat16 * out = dst + row_in_tile * FAB_LD + quarter * 16;
    if (row < rows) {
        const __nv_bfloat16 * in = src + static_cast<size_t>(row) * hidden + col0 + quarter * 16;
        #pragma unroll
        for (int i = 0; i < 2; i++) {
            *reinterpret_cast<uint4 *>(out + i * 8) =
                *reinterpret_cast<const uint4 *>(in + i * 8);
        }
    } else {
        const __nv_bfloat16 zero = __float2bfloat16_rn(0.0f);
        #pragma unroll
        for (int i = 0; i < 16; i++) {
            out[i] = zero;
        }
    }
}

static __global__ void makepad_cuda_flash_attention_bf16_d64_f32_kernel(
        const __nv_bfloat16 * __restrict__ q,
        const __nv_bfloat16 * __restrict__ k,
        const __nv_bfloat16 * __restrict__ v,
        float * __restrict__ out,
        uint32_t q_len,
        uint32_t kv_len,
        uint32_t hidden,
        float scale) {
    extern __shared__ __align__(16) char fab_smem[];
    __nv_bfloat16 * q_sh = reinterpret_cast<__nv_bfloat16 *>(fab_smem);
    __nv_bfloat16 * k_ring = reinterpret_cast<__nv_bfloat16 *>(fab_smem + FAB_SMEM_Q);
    __nv_bfloat16 * v_sh = reinterpret_cast<__nv_bfloat16 *>(
        fab_smem + FAB_SMEM_Q + FAB_SMEM_K);
    float * s_sh = reinterpret_cast<float *>(
        fab_smem + FAB_SMEM_Q + FAB_SMEM_K + FAB_SMEM_V);
    __nv_bfloat16 * p_sh = reinterpret_cast<__nv_bfloat16 *>(
        fab_smem + FAB_SMEM_Q + FAB_SMEM_K + FAB_SMEM_V + FAB_SMEM_S);
    constexpr int FAB_K_STAGE = FAB_BC * FAB_LD;

    const uint32_t q0 = blockIdx.x * FAB_BR;
    const uint32_t col0 = blockIdx.y * FAB_D;
    if (q0 >= q_len) {
        return;
    }
    const int warp = threadIdx.x >> 5;
    const int warp_row = (warp >> 1) * 16;
    const int warp_col = (warp & 1) * 32;
    const int own_row = threadIdx.x >> 2;
    const int own_sub = threadIdx.x & 3;

    float o_acc[16];
    #pragma unroll
    for (int i = 0; i < 16; i++) {
        o_acc[i] = 0.0f;
    }
    float m_row = -CUDART_INF_F;
    float l_row = 0.0f;

    const uint32_t tiles = (kv_len + FAB_BC - 1) / FAB_BC;
    makepad_cuda_fab_tile_async(k, k_ring, 0, kv_len, hidden, col0);
    makepad_cuda_fa_cp_commit();
    makepad_cuda_fab_load_tile(q, q_sh, q0, q_len, hidden, col0);
    makepad_cuda_fa_cp_wait<0>();
    __syncthreads();

    for (uint32_t tile = 0; tile < tiles; tile++) {
        const uint32_t k0 = tile * FAB_BC;
        const __nv_bfloat16 * k_sh = k_ring + (tile & 1) * FAB_K_STAGE;
        makepad_cuda_fab_tile_async(v, v_sh, k0, kv_len, hidden, col0);
        makepad_cuda_fa_cp_commit();
        if (tile + 1 < tiles) {
            makepad_cuda_fab_tile_async(
                k, k_ring + ((tile + 1) & 1) * FAB_K_STAGE,
                k0 + FAB_BC, kv_len, hidden, col0);
        }
        makepad_cuda_fa_cp_commit();

        {
            nvcuda::wmma::fragment<nvcuda::wmma::matrix_a, 16, 16, 16,
                __nv_bfloat16, nvcuda::wmma::row_major> a_frag;
            nvcuda::wmma::fragment<nvcuda::wmma::matrix_b, 16, 16, 16,
                __nv_bfloat16, nvcuda::wmma::col_major> b_frag;
            nvcuda::wmma::fragment<nvcuda::wmma::accumulator, 16, 16, 16, float> c_frag[2];
            nvcuda::wmma::fill_fragment(c_frag[0], 0.0f);
            nvcuda::wmma::fill_fragment(c_frag[1], 0.0f);
            #pragma unroll
            for (int kk = 0; kk < FAB_D / 16; kk++) {
                nvcuda::wmma::load_matrix_sync(
                    a_frag, q_sh + warp_row * FAB_LD + kk * 16, FAB_LD);
                #pragma unroll
                for (int n = 0; n < 2; n++) {
                    nvcuda::wmma::load_matrix_sync(
                        b_frag, k_sh + (warp_col + n * 16) * FAB_LD + kk * 16, FAB_LD);
                    nvcuda::wmma::mma_sync(c_frag[n], a_frag, b_frag, c_frag[n]);
                }
            }
            #pragma unroll
            for (int n = 0; n < 2; n++) {
                nvcuda::wmma::store_matrix_sync(
                    s_sh + warp_row * FAB_LDS + warp_col + n * 16,
                    c_frag[n], FAB_LDS, nvcuda::wmma::mem_row_major);
            }
        }
        __syncthreads();

        {
            const uint32_t remaining = kv_len - k0;
            const uint32_t valid = remaining < FAB_BC ? remaining : FAB_BC;
            const float * s_row = s_sh + own_row * FAB_LDS;
            float seg[16];
            float tile_max = -CUDART_INF_F;
            #pragma unroll
            for (int i = 0; i < 16; i++) {
                const uint32_t c = own_sub * 16 + i;
                const float value = c < valid ? s_row[c] * scale : -CUDART_INF_F;
                seg[i] = value;
                tile_max = value > tile_max ? value : tile_max;
            }
            tile_max = fmaxf(tile_max, __shfl_xor_sync(0xffffffffu, tile_max, 1));
            tile_max = fmaxf(tile_max, __shfl_xor_sync(0xffffffffu, tile_max, 2));
            const float m_new = m_row > tile_max ? m_row : tile_max;
            const float rescale = expf(m_row - m_new);
            float sum = 0.0f;
            __nv_bfloat16 * p_row = p_sh + own_row * FAB_LDS;
            #pragma unroll
            for (int i = 0; i < 16; i++) {
                const uint32_t c = own_sub * 16 + i;
                const float p = c < valid ? expf(seg[i] - m_new) : 0.0f;
                p_row[c] = __float2bfloat16_rn(p);
                sum += p;
            }
            sum += __shfl_xor_sync(0xffffffffu, sum, 1);
            sum += __shfl_xor_sync(0xffffffffu, sum, 2);
            l_row = l_row * rescale + sum;
            m_row = m_new;
            #pragma unroll
            for (int i = 0; i < 16; i++) {
                o_acc[i] *= rescale;
            }
        }
        __syncthreads();

        makepad_cuda_fa_cp_wait<1>();
        __syncthreads();
        {
            nvcuda::wmma::fragment<nvcuda::wmma::matrix_a, 16, 16, 16,
                __nv_bfloat16, nvcuda::wmma::row_major> a_frag;
            nvcuda::wmma::fragment<nvcuda::wmma::matrix_b, 16, 16, 16,
                __nv_bfloat16, nvcuda::wmma::row_major> b_frag;
            nvcuda::wmma::fragment<nvcuda::wmma::accumulator, 16, 16, 16, float> c_frag[2];
            nvcuda::wmma::fill_fragment(c_frag[0], 0.0f);
            nvcuda::wmma::fill_fragment(c_frag[1], 0.0f);
            #pragma unroll
            for (int kk = 0; kk < FAB_BC / 16; kk++) {
                nvcuda::wmma::load_matrix_sync(
                    a_frag, p_sh + warp_row * FAB_LDS + kk * 16, FAB_LDS);
                #pragma unroll
                for (int n = 0; n < 2; n++) {
                    nvcuda::wmma::load_matrix_sync(
                        b_frag, v_sh + kk * 16 * FAB_LD + warp_col + n * 16, FAB_LD);
                    nvcuda::wmma::mma_sync(c_frag[n], a_frag, b_frag, c_frag[n]);
                }
            }
            #pragma unroll
            for (int n = 0; n < 2; n++) {
                nvcuda::wmma::store_matrix_sync(
                    s_sh + warp_row * FAB_LDS + warp_col + n * 16,
                    c_frag[n], FAB_LDS, nvcuda::wmma::mem_row_major);
            }
        }
        __syncthreads();
        {
            const float * d_row = s_sh + own_row * FAB_LDS + own_sub * 16;
            #pragma unroll
            for (int i = 0; i < 16; i++) {
                o_acc[i] += d_row[i];
            }
        }
        __syncthreads();
        makepad_cuda_fa_cp_wait<0>();
        __syncthreads();
    }

    const uint32_t out_row = q0 + own_row;
    if (out_row < q_len) {
        const float inv = l_row > 0.0f ? 1.0f / l_row : 0.0f;
        float * dst = out + static_cast<size_t>(out_row) * hidden + col0 + own_sub * 16;
        #pragma unroll
        for (int i = 0; i < 16; i += 4) {
            float4 values;
            values.x = o_acc[i] * inv;
            values.y = o_acc[i + 1] * inv;
            values.z = o_acc[i + 2] * inv;
            values.w = o_acc[i + 3] * inv;
            *reinterpret_cast<float4 *>(dst + i) = values;
        }
    }
}

extern "C" cudaError_t makepad_cuda_flash_attention_bf16_d64_f32(
        const uint16_t * q,
        const uint16_t * k,
        const uint16_t * v,
        float * out,
        uint32_t q_len,
        uint32_t kv_len,
        uint32_t head_count,
        uint32_t hidden,
        float scale,
        cudaStream_t stream) {
    if (q_len == 0 || kv_len == 0 || head_count == 0 || hidden != head_count * FAB_D) {
        return cudaErrorInvalidValue;
    }
    static bool configured = false;
    if (!configured) {
        const cudaError_t err = cudaFuncSetAttribute(
            makepad_cuda_flash_attention_bf16_d64_f32_kernel,
            cudaFuncAttributeMaxDynamicSharedMemorySize,
            static_cast<int>(FAB_SMEM_TOTAL));
        if (err != cudaSuccess) {
            return err;
        }
        configured = true;
    }
    const dim3 block(FAB_THREADS, 1, 1);
    const dim3 grid((q_len + FAB_BR - 1) / FAB_BR, head_count, 1);
    makepad_cuda_flash_attention_bf16_d64_f32_kernel
        <<<grid, block, FAB_SMEM_TOTAL, stream>>>(
            reinterpret_cast<const __nv_bfloat16 *>(q),
            reinterpret_cast<const __nv_bfloat16 *>(k),
            reinterpret_cast<const __nv_bfloat16 *>(v),
            out, q_len, kv_len, hidden, scale);
    return cudaGetLastError();
}

// Official F.sdpa / flash-attn `mha_fwd` host: one launch over [B,H,S,D]
// fp16, online-softmax WMMA d=64 (same class as FLASH hd=64). Strides are
// in elements. Token-major [B*S, H*D] is
//   batch=S*H*D, head=D, row=H*D
// matching `view(B,S,H,D).transpose(1,2)`.
constexpr size_t SDPA_SMEM_Q = static_cast<size_t>(FAB_BR) * FAB_LD * sizeof(__half);
constexpr size_t SDPA_SMEM_K = 2 * static_cast<size_t>(FAB_BC) * FAB_LD * sizeof(__half);
constexpr size_t SDPA_SMEM_V = static_cast<size_t>(FAB_BC) * FAB_LD * sizeof(__half);
constexpr size_t SDPA_SMEM_S = static_cast<size_t>(FAB_BR) * FAB_LDS * sizeof(float);
constexpr size_t SDPA_SMEM_P = static_cast<size_t>(FAB_BR) * FAB_LDS * sizeof(__half);
constexpr size_t SDPA_SMEM_TOTAL = SDPA_SMEM_Q + SDPA_SMEM_K + SDPA_SMEM_V + SDPA_SMEM_S + SDPA_SMEM_P;

static __device__ __forceinline__ void makepad_cuda_sdpa_tile_async(
        const __half * __restrict__ src,
        __half * __restrict__ dst,
        uint32_t row0,
        uint32_t rows,
        uint32_t row_stride,
        uint32_t col0) {
    const int row_in_tile = threadIdx.x >> 2;
    const int quarter = threadIdx.x & 3;
    const uint32_t row = row0 + row_in_tile;
    const int src_bytes = row < rows ? 16 : 0;
    const size_t safe_row = row < rows ? row : 0;
    const __half * in = src + static_cast<size_t>(safe_row) * row_stride + col0 + quarter * 16;
    __half * out = dst + row_in_tile * FAB_LD + quarter * 16;
    #pragma unroll
    for (int i = 0; i < 2; i++) {
        makepad_cuda_fa_cp_async16(out + i * 8, in + i * 8, src_bytes);
    }
}

static __device__ __forceinline__ void makepad_cuda_sdpa_load_tile(
        const __half * __restrict__ src,
        __half * __restrict__ dst,
        uint32_t row0,
        uint32_t rows,
        uint32_t row_stride,
        uint32_t col0) {
    const int row_in_tile = threadIdx.x >> 2;
    const int quarter = threadIdx.x & 3;
    const uint32_t row = row0 + row_in_tile;
    __half * out = dst + row_in_tile * FAB_LD + quarter * 16;
    if (row < rows) {
        const __half * in = src + static_cast<size_t>(row) * row_stride + col0 + quarter * 16;
        #pragma unroll
        for (int i = 0; i < 2; i++) {
            *reinterpret_cast<uint4 *>(out + i * 8) =
                *reinterpret_cast<const uint4 *>(in + i * 8);
        }
    } else {
        const __half zero = __float2half_rn(0.0f);
        #pragma unroll
        for (int i = 0; i < 16; i++) {
            out[i] = zero;
        }
    }
}

static __global__ void makepad_cuda_sdpa_flash_f16_d64_kernel(
        const __half * __restrict__ q,
        const __half * __restrict__ k,
        const __half * __restrict__ v,
        __half * __restrict__ out,
        uint32_t q_len,
        uint32_t kv_len,
        uint32_t q_batch_stride,
        uint32_t k_batch_stride,
        uint32_t v_batch_stride,
        uint32_t o_batch_stride,
        uint32_t q_head_stride,
        uint32_t k_head_stride,
        uint32_t v_head_stride,
        uint32_t o_head_stride,
        uint32_t q_row_stride,
        uint32_t k_row_stride,
        uint32_t v_row_stride,
        uint32_t o_row_stride,
        float scale) {
    extern __shared__ __align__(16) char sdpa_smem[];
    __half * q_sh = reinterpret_cast<__half *>(sdpa_smem);
    __half * k_ring = reinterpret_cast<__half *>(sdpa_smem + SDPA_SMEM_Q);
    __half * v_sh = reinterpret_cast<__half *>(sdpa_smem + SDPA_SMEM_Q + SDPA_SMEM_K);
    float * s_sh = reinterpret_cast<float *>(
        sdpa_smem + SDPA_SMEM_Q + SDPA_SMEM_K + SDPA_SMEM_V);
    __half * p_sh = reinterpret_cast<__half *>(
        sdpa_smem + SDPA_SMEM_Q + SDPA_SMEM_K + SDPA_SMEM_V + SDPA_SMEM_S);
    constexpr int FAB_K_STAGE = FAB_BC * FAB_LD;

    const uint32_t q0 = blockIdx.x * FAB_BR;
    const uint32_t head = blockIdx.y;
    const uint32_t batch = blockIdx.z;
    if (q0 >= q_len) {
        return;
    }
    const __half * q_bh = q + static_cast<size_t>(batch) * q_batch_stride
        + static_cast<size_t>(head) * q_head_stride;
    const __half * k_bh = k + static_cast<size_t>(batch) * k_batch_stride
        + static_cast<size_t>(head) * k_head_stride;
    const __half * v_bh = v + static_cast<size_t>(batch) * v_batch_stride
        + static_cast<size_t>(head) * v_head_stride;
    __half * o_bh = out + static_cast<size_t>(batch) * o_batch_stride
        + static_cast<size_t>(head) * o_head_stride;

    const int warp = threadIdx.x >> 5;
    const int warp_row = (warp >> 1) * 16;
    const int warp_col = (warp & 1) * 32;
    const int own_row = threadIdx.x >> 2;
    const int own_sub = threadIdx.x & 3;

    float o_acc[16];
    #pragma unroll
    for (int i = 0; i < 16; i++) {
        o_acc[i] = 0.0f;
    }
    float m_row = -CUDART_INF_F;
    float l_row = 0.0f;

    const uint32_t tiles = (kv_len + FAB_BC - 1) / FAB_BC;
    makepad_cuda_sdpa_tile_async(k_bh, k_ring, 0, kv_len, k_row_stride, 0);
    makepad_cuda_fa_cp_commit();
    makepad_cuda_sdpa_load_tile(q_bh, q_sh, q0, q_len, q_row_stride, 0);
    makepad_cuda_fa_cp_wait<0>();
    __syncthreads();

    for (uint32_t tile = 0; tile < tiles; tile++) {
        const uint32_t k0 = tile * FAB_BC;
        const __half * k_sh = k_ring + (tile & 1) * FAB_K_STAGE;
        makepad_cuda_sdpa_tile_async(v_bh, v_sh, k0, kv_len, v_row_stride, 0);
        makepad_cuda_fa_cp_commit();
        if (tile + 1 < tiles) {
            makepad_cuda_sdpa_tile_async(
                k_bh, k_ring + ((tile + 1) & 1) * FAB_K_STAGE,
                k0 + FAB_BC, kv_len, k_row_stride, 0);
        }
        makepad_cuda_fa_cp_commit();

        {
            nvcuda::wmma::fragment<nvcuda::wmma::matrix_a, 16, 16, 16,
                __half, nvcuda::wmma::row_major> a_frag;
            nvcuda::wmma::fragment<nvcuda::wmma::matrix_b, 16, 16, 16,
                __half, nvcuda::wmma::col_major> b_frag;
            nvcuda::wmma::fragment<nvcuda::wmma::accumulator, 16, 16, 16, float> c_frag[2];
            nvcuda::wmma::fill_fragment(c_frag[0], 0.0f);
            nvcuda::wmma::fill_fragment(c_frag[1], 0.0f);
            #pragma unroll
            for (int kk = 0; kk < FAB_D / 16; kk++) {
                nvcuda::wmma::load_matrix_sync(
                    a_frag, q_sh + warp_row * FAB_LD + kk * 16, FAB_LD);
                #pragma unroll
                for (int n = 0; n < 2; n++) {
                    nvcuda::wmma::load_matrix_sync(
                        b_frag, k_sh + (warp_col + n * 16) * FAB_LD + kk * 16, FAB_LD);
                    nvcuda::wmma::mma_sync(c_frag[n], a_frag, b_frag, c_frag[n]);
                }
            }
            #pragma unroll
            for (int n = 0; n < 2; n++) {
                nvcuda::wmma::store_matrix_sync(
                    s_sh + warp_row * FAB_LDS + warp_col + n * 16,
                    c_frag[n], FAB_LDS, nvcuda::wmma::mem_row_major);
            }
        }
        __syncthreads();

        {
            const uint32_t remaining = kv_len - k0;
            const uint32_t valid = remaining < FAB_BC ? remaining : FAB_BC;
            const float * s_row = s_sh + own_row * FAB_LDS;
            float seg[16];
            float tile_max = -CUDART_INF_F;
            #pragma unroll
            for (int i = 0; i < 16; i++) {
                const uint32_t c = own_sub * 16 + i;
                const float value = c < valid ? s_row[c] * scale : -CUDART_INF_F;
                seg[i] = value;
                tile_max = value > tile_max ? value : tile_max;
            }
            tile_max = fmaxf(tile_max, __shfl_xor_sync(0xffffffffu, tile_max, 1));
            tile_max = fmaxf(tile_max, __shfl_xor_sync(0xffffffffu, tile_max, 2));
            const float m_new = m_row > tile_max ? m_row : tile_max;
            const float rescale = expf(m_row - m_new);
            float sum = 0.0f;
            __half * p_row = p_sh + own_row * FAB_LDS;
            #pragma unroll
            for (int i = 0; i < 16; i++) {
                const uint32_t c = own_sub * 16 + i;
                const float p = c < valid ? expf(seg[i] - m_new) : 0.0f;
                p_row[c] = __float2half_rn(p);
                sum += p;
            }
            sum += __shfl_xor_sync(0xffffffffu, sum, 1);
            sum += __shfl_xor_sync(0xffffffffu, sum, 2);
            l_row = l_row * rescale + sum;
            m_row = m_new;
            #pragma unroll
            for (int i = 0; i < 16; i++) {
                o_acc[i] *= rescale;
            }
        }
        __syncthreads();

        makepad_cuda_fa_cp_wait<1>();
        __syncthreads();
        {
            nvcuda::wmma::fragment<nvcuda::wmma::matrix_a, 16, 16, 16,
                __half, nvcuda::wmma::row_major> a_frag;
            nvcuda::wmma::fragment<nvcuda::wmma::matrix_b, 16, 16, 16,
                __half, nvcuda::wmma::row_major> b_frag;
            nvcuda::wmma::fragment<nvcuda::wmma::accumulator, 16, 16, 16, float> c_frag[2];
            nvcuda::wmma::fill_fragment(c_frag[0], 0.0f);
            nvcuda::wmma::fill_fragment(c_frag[1], 0.0f);
            #pragma unroll
            for (int kk = 0; kk < FAB_BC / 16; kk++) {
                nvcuda::wmma::load_matrix_sync(
                    a_frag, p_sh + warp_row * FAB_LDS + kk * 16, FAB_LDS);
                #pragma unroll
                for (int n = 0; n < 2; n++) {
                    nvcuda::wmma::load_matrix_sync(
                        b_frag, v_sh + kk * 16 * FAB_LD + warp_col + n * 16, FAB_LD);
                    nvcuda::wmma::mma_sync(c_frag[n], a_frag, b_frag, c_frag[n]);
                }
            }
            #pragma unroll
            for (int n = 0; n < 2; n++) {
                nvcuda::wmma::store_matrix_sync(
                    s_sh + warp_row * FAB_LDS + warp_col + n * 16,
                    c_frag[n], FAB_LDS, nvcuda::wmma::mem_row_major);
            }
        }
        __syncthreads();
        {
            const float * d_row = s_sh + own_row * FAB_LDS + own_sub * 16;
            #pragma unroll
            for (int i = 0; i < 16; i++) {
                o_acc[i] += d_row[i];
            }
        }
        __syncthreads();
        makepad_cuda_fa_cp_wait<0>();
        __syncthreads();
    }

    const uint32_t out_row = q0 + own_row;
    if (out_row < q_len) {
        const float inv = l_row > 0.0f ? 1.0f / l_row : 0.0f;
        __half * dst = o_bh + static_cast<size_t>(out_row) * o_row_stride + own_sub * 16;
        #pragma unroll
        for (int i = 0; i < 16; i++) {
            dst[i] = __float2half_rn(o_acc[i] * inv);
        }
    }
}

extern "C" cudaError_t makepad_cuda_sdpa_flash_f16_d64(
        const uint16_t * q,
        const uint16_t * k,
        const uint16_t * v,
        uint16_t * out,
        uint32_t batch,
        uint32_t q_len,
        uint32_t kv_len,
        uint32_t heads,
        uint32_t q_batch_stride,
        uint32_t k_batch_stride,
        uint32_t v_batch_stride,
        uint32_t o_batch_stride,
        uint32_t q_head_stride,
        uint32_t k_head_stride,
        uint32_t v_head_stride,
        uint32_t o_head_stride,
        uint32_t q_row_stride,
        uint32_t k_row_stride,
        uint32_t v_row_stride,
        uint32_t o_row_stride,
        float scale,
        cudaStream_t stream) {
    if (q_len == 0 || kv_len == 0 || heads == 0 || batch == 0) {
        return cudaErrorInvalidValue;
    }
    static bool configured = false;
    if (!configured) {
        const cudaError_t err = cudaFuncSetAttribute(
            makepad_cuda_sdpa_flash_f16_d64_kernel,
            cudaFuncAttributeMaxDynamicSharedMemorySize,
            static_cast<int>(SDPA_SMEM_TOTAL));
        if (err != cudaSuccess) {
            return err;
        }
        configured = true;
    }
    const dim3 block(FAB_THREADS, 1, 1);
    const dim3 grid((q_len + FAB_BR - 1) / FAB_BR, heads, batch);
    makepad_cuda_sdpa_flash_f16_d64_kernel
        <<<grid, block, SDPA_SMEM_TOTAL, stream>>>(
            reinterpret_cast<const __half *>(q),
            reinterpret_cast<const __half *>(k),
            reinterpret_cast<const __half *>(v),
            reinterpret_cast<__half *>(out),
            q_len, kv_len,
            q_batch_stride, k_batch_stride, v_batch_stride, o_batch_stride,
            q_head_stride, k_head_stride, v_head_stride, o_head_stride,
            q_row_stride, k_row_stride, v_row_stride, o_row_stride,
            scale);
    return cudaGetLastError();
}

// Official RA `F.sdpa`: Q/K head_dim=64, V last-dim = 2C so head_dim_v=128.
// Same WMMA / online-softmax class as d64; two V column passes share one P.
static __global__ void makepad_cuda_sdpa_flash_f16_d64v128_kernel(
        const __half * __restrict__ q,
        const __half * __restrict__ k,
        const __half * __restrict__ v,
        __half * __restrict__ o0,
        __half * __restrict__ o1,
        uint32_t q_len,
        uint32_t kv_len,
        uint32_t q_batch_stride,
        uint32_t k_batch_stride,
        uint32_t v_batch_stride,
        uint32_t o_batch_stride,
        uint32_t q_head_stride,
        uint32_t k_head_stride,
        uint32_t v_head_stride,
        uint32_t o_head_stride,
        uint32_t q_row_stride,
        uint32_t k_row_stride,
        uint32_t v_row_stride,
        uint32_t o_row_stride,
        float scale) {
    extern __shared__ __align__(16) char sdpa_smem[];
    __half * q_sh = reinterpret_cast<__half *>(sdpa_smem);
    __half * k_ring = reinterpret_cast<__half *>(sdpa_smem + SDPA_SMEM_Q);
    __half * v_sh = reinterpret_cast<__half *>(sdpa_smem + SDPA_SMEM_Q + SDPA_SMEM_K);
    float * s_sh = reinterpret_cast<float *>(
        sdpa_smem + SDPA_SMEM_Q + SDPA_SMEM_K + SDPA_SMEM_V);
    __half * p_sh = reinterpret_cast<__half *>(
        sdpa_smem + SDPA_SMEM_Q + SDPA_SMEM_K + SDPA_SMEM_V + SDPA_SMEM_S);
    constexpr int FAB_K_STAGE = FAB_BC * FAB_LD;

    const uint32_t q0 = blockIdx.x * FAB_BR;
    const uint32_t head = blockIdx.y;
    const uint32_t batch = blockIdx.z;
    if (q0 >= q_len) {
        return;
    }
    const __half * q_bh = q + static_cast<size_t>(batch) * q_batch_stride
        + static_cast<size_t>(head) * q_head_stride;
    const __half * k_bh = k + static_cast<size_t>(batch) * k_batch_stride
        + static_cast<size_t>(head) * k_head_stride;
    const __half * v_bh = v + static_cast<size_t>(batch) * v_batch_stride
        + static_cast<size_t>(head) * v_head_stride;
    __half * o0_bh = o0 + static_cast<size_t>(batch) * o_batch_stride
        + static_cast<size_t>(head) * o_head_stride;
    __half * o1_bh = o1 + static_cast<size_t>(batch) * o_batch_stride
        + static_cast<size_t>(head) * o_head_stride;

    const int warp = threadIdx.x >> 5;
    const int warp_row = (warp >> 1) * 16;
    const int warp_col = (warp & 1) * 32;
    const int own_row = threadIdx.x >> 2;
    const int own_sub = threadIdx.x & 3;

    float o_acc0[16];
    float o_acc1[16];
    #pragma unroll
    for (int i = 0; i < 16; i++) {
        o_acc0[i] = 0.0f;
        o_acc1[i] = 0.0f;
    }
    float m_row = -CUDART_INF_F;
    float l_row = 0.0f;

    const uint32_t tiles = (kv_len + FAB_BC - 1) / FAB_BC;
    makepad_cuda_sdpa_tile_async(k_bh, k_ring, 0, kv_len, k_row_stride, 0);
    makepad_cuda_fa_cp_commit();
    makepad_cuda_sdpa_load_tile(q_bh, q_sh, q0, q_len, q_row_stride, 0);
    makepad_cuda_fa_cp_wait<0>();
    __syncthreads();

    for (uint32_t tile = 0; tile < tiles; tile++) {
        const uint32_t k0 = tile * FAB_BC;
        const __half * k_sh = k_ring + (tile & 1) * FAB_K_STAGE;
        makepad_cuda_sdpa_tile_async(v_bh, v_sh, k0, kv_len, v_row_stride, 0);
        makepad_cuda_fa_cp_commit();
        if (tile + 1 < tiles) {
            makepad_cuda_sdpa_tile_async(
                k_bh, k_ring + ((tile + 1) & 1) * FAB_K_STAGE,
                k0 + FAB_BC, kv_len, k_row_stride, 0);
        }
        makepad_cuda_fa_cp_commit();

        {
            nvcuda::wmma::fragment<nvcuda::wmma::matrix_a, 16, 16, 16,
                __half, nvcuda::wmma::row_major> a_frag;
            nvcuda::wmma::fragment<nvcuda::wmma::matrix_b, 16, 16, 16,
                __half, nvcuda::wmma::col_major> b_frag;
            nvcuda::wmma::fragment<nvcuda::wmma::accumulator, 16, 16, 16, float> c_frag[2];
            nvcuda::wmma::fill_fragment(c_frag[0], 0.0f);
            nvcuda::wmma::fill_fragment(c_frag[1], 0.0f);
            #pragma unroll
            for (int kk = 0; kk < FAB_D / 16; kk++) {
                nvcuda::wmma::load_matrix_sync(
                    a_frag, q_sh + warp_row * FAB_LD + kk * 16, FAB_LD);
                #pragma unroll
                for (int n = 0; n < 2; n++) {
                    nvcuda::wmma::load_matrix_sync(
                        b_frag, k_sh + (warp_col + n * 16) * FAB_LD + kk * 16, FAB_LD);
                    nvcuda::wmma::mma_sync(c_frag[n], a_frag, b_frag, c_frag[n]);
                }
            }
            #pragma unroll
            for (int n = 0; n < 2; n++) {
                nvcuda::wmma::store_matrix_sync(
                    s_sh + warp_row * FAB_LDS + warp_col + n * 16,
                    c_frag[n], FAB_LDS, nvcuda::wmma::mem_row_major);
            }
        }
        __syncthreads();

        {
            const uint32_t remaining = kv_len - k0;
            const uint32_t valid = remaining < FAB_BC ? remaining : FAB_BC;
            const float * s_row = s_sh + own_row * FAB_LDS;
            float seg[16];
            float tile_max = -CUDART_INF_F;
            #pragma unroll
            for (int i = 0; i < 16; i++) {
                const uint32_t c = own_sub * 16 + i;
                const float value = c < valid ? s_row[c] * scale : -CUDART_INF_F;
                seg[i] = value;
                tile_max = value > tile_max ? value : tile_max;
            }
            tile_max = fmaxf(tile_max, __shfl_xor_sync(0xffffffffu, tile_max, 1));
            tile_max = fmaxf(tile_max, __shfl_xor_sync(0xffffffffu, tile_max, 2));
            const float m_new = m_row > tile_max ? m_row : tile_max;
            const float rescale = expf(m_row - m_new);
            float sum = 0.0f;
            __half * p_row = p_sh + own_row * FAB_LDS;
            #pragma unroll
            for (int i = 0; i < 16; i++) {
                const uint32_t c = own_sub * 16 + i;
                const float p = c < valid ? expf(seg[i] - m_new) : 0.0f;
                p_row[c] = __float2half_rn(p);
                sum += p;
            }
            sum += __shfl_xor_sync(0xffffffffu, sum, 1);
            sum += __shfl_xor_sync(0xffffffffu, sum, 2);
            l_row = l_row * rescale + sum;
            m_row = m_new;
            #pragma unroll
            for (int i = 0; i < 16; i++) {
                o_acc0[i] *= rescale;
                o_acc1[i] *= rescale;
            }
        }
        __syncthreads();

        makepad_cuda_fa_cp_wait<1>();
        __syncthreads();
        {
            nvcuda::wmma::fragment<nvcuda::wmma::matrix_a, 16, 16, 16,
                __half, nvcuda::wmma::row_major> a_frag;
            nvcuda::wmma::fragment<nvcuda::wmma::matrix_b, 16, 16, 16,
                __half, nvcuda::wmma::row_major> b_frag;
            nvcuda::wmma::fragment<nvcuda::wmma::accumulator, 16, 16, 16, float> c_frag[2];
            nvcuda::wmma::fill_fragment(c_frag[0], 0.0f);
            nvcuda::wmma::fill_fragment(c_frag[1], 0.0f);
            #pragma unroll
            for (int kk = 0; kk < FAB_BC / 16; kk++) {
                nvcuda::wmma::load_matrix_sync(
                    a_frag, p_sh + warp_row * FAB_LDS + kk * 16, FAB_LDS);
                #pragma unroll
                for (int n = 0; n < 2; n++) {
                    nvcuda::wmma::load_matrix_sync(
                        b_frag, v_sh + kk * 16 * FAB_LD + warp_col + n * 16, FAB_LD);
                    nvcuda::wmma::mma_sync(c_frag[n], a_frag, b_frag, c_frag[n]);
                }
            }
            #pragma unroll
            for (int n = 0; n < 2; n++) {
                nvcuda::wmma::store_matrix_sync(
                    s_sh + warp_row * FAB_LDS + warp_col + n * 16,
                    c_frag[n], FAB_LDS, nvcuda::wmma::mem_row_major);
            }
        }
        __syncthreads();
        {
            const float * d_row = s_sh + own_row * FAB_LDS + own_sub * 16;
            #pragma unroll
            for (int i = 0; i < 16; i++) {
                o_acc0[i] += d_row[i];
            }
        }
        __syncthreads();

        makepad_cuda_sdpa_tile_async(v_bh, v_sh, k0, kv_len, v_row_stride, 64);
        makepad_cuda_fa_cp_commit();
        makepad_cuda_fa_cp_commit();
        makepad_cuda_fa_cp_wait<1>();
        __syncthreads();
        {
            nvcuda::wmma::fragment<nvcuda::wmma::matrix_a, 16, 16, 16,
                __half, nvcuda::wmma::row_major> a_frag;
            nvcuda::wmma::fragment<nvcuda::wmma::matrix_b, 16, 16, 16,
                __half, nvcuda::wmma::row_major> b_frag;
            nvcuda::wmma::fragment<nvcuda::wmma::accumulator, 16, 16, 16, float> c_frag[2];
            nvcuda::wmma::fill_fragment(c_frag[0], 0.0f);
            nvcuda::wmma::fill_fragment(c_frag[1], 0.0f);
            #pragma unroll
            for (int kk = 0; kk < FAB_BC / 16; kk++) {
                nvcuda::wmma::load_matrix_sync(
                    a_frag, p_sh + warp_row * FAB_LDS + kk * 16, FAB_LDS);
                #pragma unroll
                for (int n = 0; n < 2; n++) {
                    nvcuda::wmma::load_matrix_sync(
                        b_frag, v_sh + kk * 16 * FAB_LD + warp_col + n * 16, FAB_LD);
                    nvcuda::wmma::mma_sync(c_frag[n], a_frag, b_frag, c_frag[n]);
                }
            }
            #pragma unroll
            for (int n = 0; n < 2; n++) {
                nvcuda::wmma::store_matrix_sync(
                    s_sh + warp_row * FAB_LDS + warp_col + n * 16,
                    c_frag[n], FAB_LDS, nvcuda::wmma::mem_row_major);
            }
        }
        __syncthreads();
        {
            const float * d_row = s_sh + own_row * FAB_LDS + own_sub * 16;
            #pragma unroll
            for (int i = 0; i < 16; i++) {
                o_acc1[i] += d_row[i];
            }
        }
        __syncthreads();
        makepad_cuda_fa_cp_wait<0>();
        __syncthreads();
    }

    const uint32_t out_row = q0 + own_row;
    if (out_row < q_len) {
        const float inv = l_row > 0.0f ? 1.0f / l_row : 0.0f;
        __half * dst0 = o0_bh + static_cast<size_t>(out_row) * o_row_stride + own_sub * 16;
        __half * dst1 = o1_bh + static_cast<size_t>(out_row) * o_row_stride + own_sub * 16;
        #pragma unroll
        for (int i = 0; i < 16; i++) {
            dst0[i] = __float2half_rn(o_acc0[i] * inv);
            dst1[i] = __float2half_rn(o_acc1[i] * inv);
        }
    }
}

extern "C" cudaError_t makepad_cuda_sdpa_flash_f16_d64v128(
        const uint16_t * q,
        const uint16_t * k,
        const uint16_t * v,
        uint16_t * o0,
        uint16_t * o1,
        uint32_t batch,
        uint32_t q_len,
        uint32_t kv_len,
        uint32_t heads,
        uint32_t q_batch_stride,
        uint32_t k_batch_stride,
        uint32_t v_batch_stride,
        uint32_t o_batch_stride,
        uint32_t q_head_stride,
        uint32_t k_head_stride,
        uint32_t v_head_stride,
        uint32_t o_head_stride,
        uint32_t q_row_stride,
        uint32_t k_row_stride,
        uint32_t v_row_stride,
        uint32_t o_row_stride,
        float scale,
        cudaStream_t stream) {
    if (q_len == 0 || kv_len == 0 || heads == 0 || batch == 0) {
        return cudaErrorInvalidValue;
    }
    static bool configured = false;
    if (!configured) {
        const cudaError_t err = cudaFuncSetAttribute(
            makepad_cuda_sdpa_flash_f16_d64v128_kernel,
            cudaFuncAttributeMaxDynamicSharedMemorySize,
            static_cast<int>(SDPA_SMEM_TOTAL));
        if (err != cudaSuccess) {
            return err;
        }
        configured = true;
    }
    const dim3 block(FAB_THREADS, 1, 1);
    const dim3 grid((q_len + FAB_BR - 1) / FAB_BR, heads, batch);
    makepad_cuda_sdpa_flash_f16_d64v128_kernel
        <<<grid, block, SDPA_SMEM_TOTAL, stream>>>(
            reinterpret_cast<const __half *>(q),
            reinterpret_cast<const __half *>(k),
            reinterpret_cast<const __half *>(v),
            reinterpret_cast<__half *>(o0),
            reinterpret_cast<__half *>(o1),
            q_len, kv_len,
            q_batch_stride, k_batch_stride, v_batch_stride, o_batch_stride,
            q_head_stride, k_head_stride, v_head_stride, o_head_stride,
            q_row_stride, k_row_stride, v_row_stride, o_row_stride,
            scale);
    return cudaGetLastError();
}

// ---------------------------------------------------------------------------
// Col-major (m_chunk x k_total) f16 im2col slab for a stride-1 "same" planar
// conv: column kk = (ic*kh + ky)*kw + kx (ic-major, so the kh*kw taps of one
// input plane are adjacent columns and L2 absorbs the re-reads), row = output
// pixel p0+row. Zero where a tap falls outside the plane. Feeding this to one
// plain f16 gemm per chunk replaces the 9-shift accumulator recipe whose
// padded f32 accumulator was read+written per shift.
static __global__ void makepad_cuda_im2col_planar_f32_to_f16_kernel(
        const float * __restrict__ input,
        __half * __restrict__ output,
        uint32_t width,
        uint32_t height,
        uint32_t kw,
        uint32_t kh,
        uint32_t pad_x,
        uint32_t pad_y,
        uint32_t p0,
        uint32_t m_chunk) {
    // One grid column per im2col column: the tap decomposition divides are
    // block-uniform, and per element only the y = p / width divide remains
    // (the flat-index formulation cost two 64-bit div/mods per element).
    const uint32_t col = blockIdx.y;
    const uint32_t row = blockIdx.x * blockDim.x + threadIdx.x;
    if (row >= m_chunk) {
        return;
    }
    const uint32_t kx = col % kw;
    const uint32_t rest = col / kw;
    const uint32_t ky = rest % kh;
    const uint32_t ic = rest / kh;
    const uint32_t p = p0 + row;
    const uint32_t y = p / width;
    const uint32_t x = p - y * width;
    const uint32_t sy = y + ky - pad_y; // unsigned wrap == out of range
    const uint32_t sx = x + kx - pad_x;
    float value = 0.0f;
    if (sy < height && sx < width) {
        value = input[(static_cast<size_t>(ic) * height + sy) * width + sx];
    }
    output[static_cast<size_t>(col) * m_chunk + row] = __float2half_rn(value);
}

extern "C" cudaError_t makepad_cuda_im2col_planar_f32_to_f16(
        const float * input,
        uint16_t * output,
        uint32_t width,
        uint32_t height,
        uint32_t kw,
        uint32_t kh,
        uint32_t pad_x,
        uint32_t pad_y,
        uint32_t p0,
        uint32_t m_chunk,
        uint32_t k_total,
        cudaStream_t stream) {
    if (m_chunk == 0 || k_total == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid((m_chunk + block.x - 1) / block.x, k_total, 1);
    makepad_cuda_im2col_planar_f32_to_f16_kernel<<<grid, block, 0, stream>>>(
        input, reinterpret_cast<__half *>(output), width, height, kw, kh,
        pad_x, pad_y, p0, m_chunk);
    return cudaGetLastError();
}

// ---------------------------------------------------------------------------
// Multi-block group norm statistics: the single-block-per-group stats kernel
// serialized ~4M elements per block at the big decode planes. Stage 1 spreads
// each group over `chunk_count` blocks producing f64 partial sums; stage 2
// combines them into the same (mean, inv_std) f32 stats the apply kernel
// consumes. f64 accumulation matches the single-block kernel's precision.
static __device__ __forceinline__ void makepad_cuda_gn_reduce_f64_pair(
        double * shared_sum,
        double * shared_sum_sq,
        double sum,
        double sum_sq) {
    shared_sum[threadIdx.x] = sum;
    shared_sum_sq[threadIdx.x] = sum_sq;
    __syncthreads();
    for (uint32_t stride = blockDim.x / 2; stride > 0; stride >>= 1) {
        if (threadIdx.x < stride) {
            shared_sum[threadIdx.x] += shared_sum[threadIdx.x + stride];
            shared_sum_sq[threadIdx.x] += shared_sum_sq[threadIdx.x + stride];
        }
        __syncthreads();
    }
}

static __global__ void makepad_cuda_group_norm_planar_partials_kernel(
        const float * __restrict__ input,
        double * __restrict__ partials, // [group][chunk] -> (sum, sum_sq)
        uint32_t plane,
        uint32_t channels_per_group,
        uint32_t chunk_count) {
    const uint32_t group = blockIdx.x;
    const uint32_t chunk = blockIdx.y;
    const size_t group_elems = static_cast<size_t>(plane) * channels_per_group;
    const size_t chunk_len = (group_elems + chunk_count - 1) / chunk_count;
    const size_t begin = static_cast<size_t>(chunk) * chunk_len;
    size_t end = begin + chunk_len;
    if (end > group_elems) {
        end = group_elems;
    }
    const float * group_in = input + static_cast<size_t>(group) * group_elems;
    double sum = 0.0;
    double sum_sq = 0.0;
    for (size_t idx = begin + threadIdx.x; idx < end; idx += blockDim.x) {
        const double value = static_cast<double>(group_in[idx]);
        sum += value;
        sum_sq += value * value;
    }
    __shared__ double shared_sum[256];
    __shared__ double shared_sum_sq[256];
    makepad_cuda_gn_reduce_f64_pair(shared_sum, shared_sum_sq, sum, sum_sq);
    if (threadIdx.x == 0) {
        const size_t slot = (static_cast<size_t>(group) * chunk_count + chunk) * 2;
        partials[slot] = shared_sum[0];
        partials[slot + 1] = shared_sum_sq[0];
    }
}

static __global__ void makepad_cuda_group_norm_planar_combine_kernel(
        const double * __restrict__ partials,
        float * __restrict__ stats, // [group] -> (mean, inv_std)
        uint32_t chunk_count,
        uint32_t plane,
        uint32_t channels_per_group,
        float eps) {
    const uint32_t group = blockIdx.x;
    double sum = 0.0;
    double sum_sq = 0.0;
    for (uint32_t idx = threadIdx.x; idx < chunk_count; idx += blockDim.x) {
        const size_t slot = (static_cast<size_t>(group) * chunk_count + idx) * 2;
        sum += partials[slot];
        sum_sq += partials[slot + 1];
    }
    __shared__ double shared_sum[256];
    __shared__ double shared_sum_sq[256];
    makepad_cuda_gn_reduce_f64_pair(shared_sum, shared_sum_sq, sum, sum_sq);
    if (threadIdx.x == 0) {
        const double count = static_cast<double>(plane) * channels_per_group;
        const float mean = static_cast<float>(shared_sum[0] / count);
        const float variance = static_cast<float>(shared_sum_sq[0] / count) - mean * mean;
        stats[group * 2] = mean;
        stats[group * 2 + 1] = rsqrtf(variance + eps);
    }
}

extern "C" cudaError_t makepad_cuda_group_norm_planar_multi_f32(
        const float * input,
        const float * gamma,
        const float * beta,
        double * partials,
        float * stats,
        float * output,
        uint32_t width,
        uint32_t height,
        uint32_t channels,
        uint32_t groups,
        uint32_t chunk_count,
        float eps,
        cudaStream_t stream) {
    if (width == 0 || height == 0 || channels == 0 || groups == 0 || chunk_count == 0) {
        return cudaSuccess;
    }
    if (channels % groups != 0) {
        return cudaErrorInvalidValue;
    }
    const uint32_t plane = width * height;
    const uint32_t channels_per_group = channels / groups;
    const dim3 stats_grid(groups, chunk_count, 1);
    makepad_cuda_group_norm_planar_partials_kernel<<<stats_grid, 256, 0, stream>>>(
        input, partials, plane, channels_per_group, chunk_count);
    makepad_cuda_group_norm_planar_combine_kernel<<<groups, 256, 0, stream>>>(
        partials, stats, chunk_count, plane, channels_per_group, eps);
    const size_t total = static_cast<size_t>(plane) * channels;
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<unsigned int>((total + block.x - 1) / block.x), 1, 1);
    makepad_cuda_group_norm_planar_apply_kernel<<<grid, block, 0, stream>>>(
        input, gamma, beta, stats, output, plane, channels, channels_per_group);
    return cudaGetLastError();
}

// ---------------------------------------------------------------------------
// MiniMax H3 kernels: rotate-half RoPE with a partial rotary span, per-row
// AdaLN-table modulation (RMS norm + indexed scale/shift, indexed gated
// residual), and the value-first SwiGLU split. The AdaLN table is the
// per-block (num_timesteps * 3, 6 * hidden) modulation matrix; every row of
// the packed sequence selects its table row through idx[row] (timestep index
// * 3 + modality tag, computed host-side).
// ---------------------------------------------------------------------------

// Rotate-half RoPE: the leading `2 * rot_half` channels of every head rotate,
// the rest pass through. cos/sin tables are [token][rot_half]; both rotated
// halves share the same table entry (the reference duplicates the frequency
// block, so cos[i + rot_half] == cos[i]).
static __global__ void makepad_cuda_rope_half_f32_kernel(
        const float * __restrict__ input,
        const float * __restrict__ cos_table,
        const float * __restrict__ sin_table,
        float * __restrict__ output,
        uint32_t token_count,
        uint32_t head_count,
        uint32_t head_dim,
        uint32_t rot_half) {
    const uint32_t token = blockIdx.x;
    const uint32_t head = blockIdx.y;
    if (token >= token_count || head >= head_count) {
        return;
    }
    const size_t base = (static_cast<size_t>(token) * head_count + head)
        * static_cast<size_t>(head_dim);
    const size_t table_base = static_cast<size_t>(token) * rot_half;
    for (uint32_t i = threadIdx.x; i < rot_half; i += blockDim.x) {
        const float cos_v = cos_table[table_base + i];
        const float sin_v = sin_table[table_base + i];
        const float x1 = input[base + i];
        const float x2 = input[base + rot_half + i];
        output[base + i] = x1 * cos_v - x2 * sin_v;
        output[base + rot_half + i] = x2 * cos_v + x1 * sin_v;
    }
    for (uint32_t i = 2 * rot_half + threadIdx.x; i < head_dim; i += blockDim.x) {
        output[base + i] = input[base + i];
    }
}

// Qwen's BF16 RoPE operator rounds BOTH products to BF16 before their
// addition/subtraction, then rounds the sum. A single fused f32 expression
// followed by one BF16 cast differs materially at long positions.
static __global__ void makepad_cuda_rope_half_bf16_f32_kernel(
        const float * __restrict__ input,
        const float * __restrict__ cos_table,
        const float * __restrict__ sin_table,
        float * __restrict__ output,
        uint32_t token_count,
        uint32_t head_count,
        uint32_t head_dim,
        uint32_t rot_half) {
    const uint32_t token = blockIdx.x;
    const uint32_t head = blockIdx.y;
    if (token >= token_count || head >= head_count) {
        return;
    }
    const size_t base = (static_cast<size_t>(token) * head_count + head)
        * static_cast<size_t>(head_dim);
    const size_t table_base = static_cast<size_t>(token) * rot_half;
    for (uint32_t i = threadIdx.x; i < rot_half; i += blockDim.x) {
        const float cos_v = cos_table[table_base + i];
        const float sin_v = sin_table[table_base + i];
        const float x1 = input[base + i];
        const float x2 = input[base + rot_half + i];
        const float x1_cos = __bfloat162float(__float2bfloat16_rn(x1 * cos_v));
        const float x2_sin = __bfloat162float(__float2bfloat16_rn(x2 * sin_v));
        const float x2_cos = __bfloat162float(__float2bfloat16_rn(x2 * cos_v));
        const float x1_sin = __bfloat162float(__float2bfloat16_rn(x1 * sin_v));
        output[base + i] = __bfloat162float(__float2bfloat16_rn(x1_cos - x2_sin));
        output[base + rot_half + i] =
            __bfloat162float(__float2bfloat16_rn(x2_cos + x1_sin));
    }
    for (uint32_t i = 2 * rot_half + threadIdx.x; i < head_dim; i += blockDim.x) {
        output[base + i] = input[base + i];
    }
}

extern "C" cudaError_t makepad_cuda_rope_half_f32(
        const float * input,
        const float * cos_table,
        const float * sin_table,
        float * output,
        uint32_t token_count,
        uint32_t head_count,
        uint32_t head_dim,
        uint32_t rot_half,
        cudaStream_t stream) {
    if (token_count == 0 || head_count == 0 || rot_half == 0) {
        return cudaSuccess;
    }
    const dim3 block(rot_half < 128 ? 64 : 128, 1, 1);
    const dim3 grid(token_count, head_count, 1);
    makepad_cuda_rope_half_f32_kernel<<<grid, block, 0, stream>>>(
        input, cos_table, sin_table, output, token_count, head_count, head_dim, rot_half);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_cuda_rope_half_bf16_f32(
        const float * input,
        const float * cos_table,
        const float * sin_table,
        float * output,
        uint32_t token_count,
        uint32_t head_count,
        uint32_t head_dim,
        uint32_t rot_half,
        cudaStream_t stream) {
    if (token_count == 0 || head_count == 0 || rot_half == 0) {
        return cudaSuccess;
    }
    const dim3 block(rot_half < 128 ? 64 : 128, 1, 1);
    const dim3 grid(token_count, head_count, 1);
    makepad_cuda_rope_half_bf16_f32_kernel<<<grid, block, 0, stream>>>(
        input, cos_table, sin_table, output, token_count, head_count, head_dim, rot_half);
    return cudaGetLastError();
}

static __global__ void makepad_cuda_rope_half_f16_kernel(
        const __half * __restrict__ input,
        const float * __restrict__ cos_table,
        const float * __restrict__ sin_table,
        __half * __restrict__ output,
        uint32_t token_count,
        uint32_t head_count,
        uint32_t head_dim,
        uint32_t rot_half) {
    const uint32_t token = blockIdx.x;
    const uint32_t head = blockIdx.y;
    if (token >= token_count || head >= head_count) {
        return;
    }
    const size_t base = (static_cast<size_t>(token) * head_count + head)
        * static_cast<size_t>(head_dim);
    const size_t table_base = static_cast<size_t>(token) * rot_half;
    for (uint32_t i = threadIdx.x; i < rot_half; i += blockDim.x) {
        const float cos_v = cos_table[table_base + i];
        const float sin_v = sin_table[table_base + i];
        const float x1 = __half2float(input[base + i]);
        const float x2 = __half2float(input[base + rot_half + i]);
        output[base + i] = __float2half(x1 * cos_v - x2 * sin_v);
        output[base + rot_half + i] = __float2half(x2 * cos_v + x1 * sin_v);
    }
    for (uint32_t i = 2 * rot_half + threadIdx.x; i < head_dim; i += blockDim.x) {
        output[base + i] = input[base + i];
    }
}

extern "C" cudaError_t makepad_cuda_rope_half_f16(
        const uint16_t * input,
        const float * cos_table,
        const float * sin_table,
        uint16_t * output,
        uint32_t token_count,
        uint32_t head_count,
        uint32_t head_dim,
        uint32_t rot_half,
        cudaStream_t stream) {
    if (token_count == 0 || head_count == 0 || rot_half == 0) {
        return cudaSuccess;
    }
    const dim3 block(rot_half < 128 ? 64 : 128, 1, 1);
    const dim3 grid(token_count, head_count, 1);
    makepad_cuda_rope_half_f16_kernel<<<grid, block, 0, stream>>>(
        reinterpret_cast<const __half *>(input), cos_table, sin_table,
        reinterpret_cast<__half *>(output), token_count, head_count, head_dim, rot_half);
    return cudaGetLastError();
}

// RMS norm (weighted) + per-row indexed AdaLN modulation:
// y = rmsnorm(x) * w * (1 + table[idx[row]*stride + scale_off + c])
//   + table[idx[row]*stride + shift_off + c].
// One block per row; f32 math, f32 or f16 output.
template <typename OutT>
static __global__ void makepad_cuda_rms_norm_mod_indexed_kernel(
        const float * __restrict__ input,
        const float * __restrict__ weight,
        const float * __restrict__ table,
        const uint32_t * __restrict__ idx,
        OutT * __restrict__ output,
        uint32_t cols,
        uint32_t table_stride,
        uint32_t scale_off,
        uint32_t shift_off,
        float eps) {
    const uint32_t row = blockIdx.x;
    const float * x = input + static_cast<size_t>(row) * cols;
    OutT * y = output + static_cast<size_t>(row) * cols;
    const size_t t = static_cast<size_t>(idx[row]) * table_stride;
    const float * scale = table + t + scale_off;
    const float * shift = table + t + shift_off;

    float sumsq = 0.0f;
    for (uint32_t c = threadIdx.x; c < cols; c += blockDim.x) {
        const float v = x[c];
        sumsq += v * v;
    }
    __shared__ float partials[32];
    for (int offset = 16; offset > 0; offset >>= 1) {
        sumsq += __shfl_down_sync(0xffffffffu, sumsq, offset);
    }
    const int warp = threadIdx.x >> 5;
    const int lane = threadIdx.x & 31;
    if (lane == 0) {
        partials[warp] = sumsq;
    }
    __syncthreads();
    const int warp_count = (blockDim.x + 31) >> 5;
    if (warp == 0) {
        float total = lane < warp_count ? partials[lane] : 0.0f;
        for (int offset = 16; offset > 0; offset >>= 1) {
            total += __shfl_down_sync(0xffffffffu, total, offset);
        }
        if (lane == 0) {
            partials[0] = total;
        }
    }
    __syncthreads();
    const float inv_rms = rsqrtf(partials[0] / static_cast<float>(cols) + eps);
    for (uint32_t c = threadIdx.x; c < cols; c += blockDim.x) {
        const float normed = x[c] * inv_rms * weight[c];
        const float v = normed * (1.0f + scale[c]) + shift[c];
        if constexpr (sizeof(OutT) == 2) {
            y[c] = __float2half(v);
        } else {
            y[c] = v;
        }
    }
}

extern "C" cudaError_t makepad_cuda_rms_norm_mod_indexed_f32(
        const float * input,
        const float * weight,
        const float * table,
        const uint32_t * idx,
        float * output,
        uint32_t rows,
        uint32_t cols,
        uint32_t table_stride,
        uint32_t scale_off,
        uint32_t shift_off,
        float eps,
        cudaStream_t stream) {
    if (rows == 0 || cols == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid(rows, 1, 1);
    makepad_cuda_rms_norm_mod_indexed_kernel<float><<<grid, block, 0, stream>>>(
        input, weight, table, idx, output, cols, table_stride, scale_off, shift_off, eps);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_cuda_rms_norm_mod_indexed_out16(
        const float * input,
        const float * weight,
        const float * table,
        const uint32_t * idx,
        uint16_t * output,
        uint32_t rows,
        uint32_t cols,
        uint32_t table_stride,
        uint32_t scale_off,
        uint32_t shift_off,
        float eps,
        cudaStream_t stream) {
    if (rows == 0 || cols == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid(rows, 1, 1);
    makepad_cuda_rms_norm_mod_indexed_kernel<__half><<<grid, block, 0, stream>>>(
        input, weight, table, idx, reinterpret_cast<__half *>(output),
        cols, table_stride, scale_off, shift_off, eps);
    return cudaGetLastError();
}

// residual + table[idx[row]*stride + gate_off + c] * update, all f32.
static __global__ void makepad_cuda_gated_residual_indexed_f32_kernel(
        const float * __restrict__ residual,
        const float * __restrict__ update,
        const float * __restrict__ table,
        const uint32_t * __restrict__ idx,
        float * __restrict__ output,
        uint32_t cols,
        uint32_t table_stride,
        uint32_t gate_off) {
    const uint32_t row = blockIdx.x;
    const size_t base = static_cast<size_t>(row) * cols;
    const float * gate = table + static_cast<size_t>(idx[row]) * table_stride + gate_off;
    for (uint32_t c = threadIdx.x; c < cols; c += blockDim.x) {
        output[base + c] = residual[base + c] + gate[c] * update[base + c];
    }
}

extern "C" cudaError_t makepad_cuda_gated_residual_indexed_f32(
        const float * residual,
        const float * update,
        const float * table,
        const uint32_t * idx,
        float * output,
        uint32_t rows,
        uint32_t cols,
        uint32_t table_stride,
        uint32_t gate_off,
        cudaStream_t stream) {
    if (rows == 0 || cols == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid(rows, 1, 1);
    makepad_cuda_gated_residual_indexed_f32_kernel<<<grid, block, 0, stream>>>(
        residual, update, table, idx, output, cols, table_stride, gate_off);
    return cudaGetLastError();
}

// Value-first SwiGLU split on row-major (rows, 2n) data:
// out[row, c] = x[row, c] * silu(x[row, n + c]). (The qwen swiglu_split
// kernels are gate-first; the diffusers SwiGLU is value-first.)
static __global__ void makepad_cuda_swiglu_value_gate_f32_kernel(
        const float * __restrict__ input,
        float * __restrict__ output,
        uint32_t n,
        uint32_t total) {
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx >= total) {
        return;
    }
    const uint32_t row = idx / n;
    const uint32_t inner = idx - row * n;
    const size_t base = static_cast<size_t>(row) * (2u * n);
    const float value = input[base + inner];
    const float gate = input[base + n + inner];
    const float s = gate / (1.0f + expf(-gate));
    output[idx] = value * s;
}

extern "C" cudaError_t makepad_cuda_swiglu_value_gate_f32(
        const float * input,
        float * output,
        uint32_t rows,
        uint32_t n,
        cudaStream_t stream) {
    const uint64_t total64 = static_cast<uint64_t>(rows) * n;
    if (total64 == 0 || total64 > 0xffffffffull) {
        return total64 == 0 ? cudaSuccess : cudaErrorInvalidValue;
    }
    const uint32_t total = static_cast<uint32_t>(total64);
    const dim3 block(256, 1, 1);
    const dim3 grid((total + block.x - 1) / block.x, 1, 1);
    makepad_cuda_swiglu_value_gate_f32_kernel<<<grid, block, 0, stream>>>(
        input, output, n, total);
    return cudaGetLastError();
}

// ---- bf16 activation spine (flux2) ------------------------------------
// The bf16 GEMM path already rounds every linear input RN-even and writes
// bf16 outputs; the f32 detours between two linears are pure storage
// transforms of already-rounded values. These kernels keep such segments in
// bf16 storage end to end — bit-identical values, roughly half the traffic
// and none of the standalone conversion passes.

// LayerNorm+mod with the RN-even bf16 store the next linear's staging would
// have applied. Same mean/variance arithmetic as the f32 kernel above.
static __global__ void makepad_cuda_layer_norm_mul_add_f32_out_bf16_kernel(
        const float * __restrict__ input,
        const float * __restrict__ gamma,
        const float * __restrict__ beta,
        uint16_t * __restrict__ output,
        uint32_t row_count,
        uint32_t cols,
        float eps,
        float gamma_add) {
    const uint32_t row = blockIdx.x;
    if (row >= row_count) {
        return;
    }
    const float * row_in = input + static_cast<size_t>(row) * cols;
    uint16_t * row_out = output + static_cast<size_t>(row) * cols;

    float sum = 0.0f;
    for (uint32_t idx = threadIdx.x; idx < cols; idx += blockDim.x) {
        sum += row_in[idx];
    }
    sum = makepad_cuda_diff_block_reduce_sum(sum);
    __shared__ float shared_mean;
    __shared__ float shared_inv;
    if (threadIdx.x == 0) {
        shared_mean = sum / static_cast<float>(cols);
    }
    __syncthreads();

    const float mean = shared_mean;
    float sq_sum = 0.0f;
    for (uint32_t idx = threadIdx.x; idx < cols; idx += blockDim.x) {
        const float centered = row_in[idx] - mean;
        sq_sum += centered * centered;
    }
    sq_sum = makepad_cuda_diff_block_reduce_sum(sq_sum);
    if (threadIdx.x == 0) {
        shared_inv = rsqrtf(sq_sum / static_cast<float>(cols) + eps);
    }
    __syncthreads();

    const float inv = shared_inv;
    for (uint32_t idx = threadIdx.x; idx < cols; idx += blockDim.x) {
        const __nv_bfloat16 h = __float2bfloat16_rn(
            (row_in[idx] - mean) * inv * (gamma[idx] + gamma_add) + beta[idx]);
        row_out[idx] = *reinterpret_cast<const uint16_t *>(&h);
    }
}

extern "C" cudaError_t makepad_cuda_layer_norm_mul_add_f32_out_bf16(
        const float * input,
        const float * gamma,
        const float * beta,
        uint16_t * output,
        uint32_t row_count,
        uint32_t cols,
        float eps,
        float gamma_add,
        cudaStream_t stream) {
    if (row_count == 0 || cols == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid(row_count, 1, 1);
    makepad_cuda_layer_norm_mul_add_f32_out_bf16_kernel<<<grid, block, 0, stream>>>(
        input, gamma, beta, output, row_count, cols, eps, gamma_add);
    return cudaGetLastError();
}

// Expand a bf16 column slab into a contiguous f32 tensor (lossless).
static __global__ void makepad_cuda_bf16_slab_to_f32_kernel(
        const uint16_t * __restrict__ input,
        float * __restrict__ output,
        uint32_t in_stride,
        uint32_t col_off,
        uint32_t cols,
        uint32_t total) {
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx >= total) {
        return;
    }
    const uint32_t row = idx / cols;
    const uint32_t inner = idx - row * cols;
    const uint16_t bits = input[static_cast<size_t>(row) * in_stride + col_off + inner];
    output[idx] = __bfloat162float(*reinterpret_cast<const __nv_bfloat16 *>(&bits));
}

extern "C" cudaError_t makepad_cuda_bf16_slab_to_f32(
        const uint16_t * input,
        float * output,
        uint32_t rows,
        uint32_t in_stride,
        uint32_t col_off,
        uint32_t cols,
        cudaStream_t stream) {
    if (col_off + cols > in_stride && rows > 0) {
        return cudaErrorInvalidValue;
    }
    const uint64_t total64 = static_cast<uint64_t>(rows) * cols;
    if (total64 == 0 || total64 > 0xffffffffull) {
        return total64 == 0 ? cudaSuccess : cudaErrorInvalidValue;
    }
    const uint32_t total = static_cast<uint32_t>(total64);
    const dim3 block(256, 1, 1);
    const dim3 grid((total + block.x - 1) / block.x, 1, 1);
    makepad_cuda_bf16_slab_to_f32_kernel<<<grid, block, 0, stream>>>(
        input, output, in_stride, col_off, cols, total);
    return cudaGetLastError();
}

// Gate-first SwiGLU over a bf16 slab, storing bf16-RN (the bits the next
// linear's staging would produce from the f32 result). Same silu/mul
// arithmetic as the f32 kernels.
static __global__ void makepad_cuda_swiglu_gate_first_bf16slab_kernel(
        const uint16_t * __restrict__ input,
        uint16_t * __restrict__ output,
        uint32_t in_stride,
        uint32_t gate_offset,
        uint32_t n,
        uint32_t total) {
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx >= total) {
        return;
    }
    const uint32_t row = idx / n;
    const uint32_t inner = idx - row * n;
    const size_t base = static_cast<size_t>(row) * in_stride + gate_offset;
    const uint16_t gate_bits = input[base + inner];
    const uint16_t value_bits = input[base + n + inner];
    const float gate = __bfloat162float(*reinterpret_cast<const __nv_bfloat16 *>(&gate_bits));
    const float value = __bfloat162float(*reinterpret_cast<const __nv_bfloat16 *>(&value_bits));
    const float s = gate / (1.0f + expf(-gate));
    const __nv_bfloat16 h = __float2bfloat16_rn(value * s);
    output[idx] = *reinterpret_cast<const uint16_t *>(&h);
}

extern "C" cudaError_t makepad_cuda_swiglu_gate_first_bf16slab(
        const uint16_t * input,
        uint16_t * output,
        uint32_t rows,
        uint32_t in_stride,
        uint32_t gate_offset,
        uint32_t n,
        cudaStream_t stream) {
    if (gate_offset + 2u * n > in_stride && rows > 0) {
        return cudaErrorInvalidValue;
    }
    const uint64_t total64 = static_cast<uint64_t>(rows) * n;
    if (total64 == 0 || total64 > 0xffffffffull) {
        return total64 == 0 ? cudaSuccess : cudaErrorInvalidValue;
    }
    const uint32_t total = static_cast<uint32_t>(total64);
    const dim3 block(256, 1, 1);
    const dim3 grid((total + block.x - 1) / block.x, 1, 1);
    makepad_cuda_swiglu_gate_first_bf16slab_kernel<<<grid, block, 0, stream>>>(
        input, output, in_stride, gate_offset, n, total);
    return cudaGetLastError();
}

// out[r] = [rn_bf16(a[r]) | b[r]]: the [attn | mlp_act] concat staged
// straight into the down-projection's bf16 input layout.
static __global__ void makepad_cuda_concat_f32rn_bf16_kernel(
        const float * __restrict__ a,
        const uint16_t * __restrict__ b,
        uint16_t * __restrict__ output,
        uint32_t a_cols,
        uint32_t b_cols,
        uint32_t total) {
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx >= total) {
        return;
    }
    const uint32_t out_cols = a_cols + b_cols;
    const uint32_t row = idx / out_cols;
    const uint32_t inner = idx - row * out_cols;
    if (inner < a_cols) {
        const __nv_bfloat16 h =
            __float2bfloat16_rn(a[static_cast<size_t>(row) * a_cols + inner]);
        output[idx] = *reinterpret_cast<const uint16_t *>(&h);
    } else {
        output[idx] = b[static_cast<size_t>(row) * b_cols + (inner - a_cols)];
    }
}

extern "C" cudaError_t makepad_cuda_concat_f32rn_bf16(
        const float * a,
        const uint16_t * b,
        uint16_t * output,
        uint32_t rows,
        uint32_t a_cols,
        uint32_t b_cols,
        cudaStream_t stream) {
    const uint64_t total64 = static_cast<uint64_t>(rows) * (a_cols + b_cols);
    if (total64 == 0 || total64 > 0xffffffffull) {
        return total64 == 0 ? cudaSuccess : cudaErrorInvalidValue;
    }
    const uint32_t total = static_cast<uint32_t>(total64);
    const dim3 block(256, 1, 1);
    const dim3 grid((total + block.x - 1) / block.x, 1, 1);
    makepad_cuda_concat_f32rn_bf16_kernel<<<grid, block, 0, stream>>>(
        a, b, output, a_cols, b_cols, total);
    return cudaGetLastError();
}

// Gate-first SwiGLU reading a column slab of a wider row-major buffer in
// place: out[row, c] = silu(x[row, off + c]) * x[row, off + n + c], with the
// exact silu/mul arithmetic of the value_gate kernel above. Replaces the
// slice+slice+swap-concat dance the flux2 DiT paid on every MLP: 4 launches
// and ~3 full-tensor memory passes become one strided read.
static __global__ void makepad_cuda_swiglu_gate_first_strided_f32_kernel(
        const float * __restrict__ input,
        float * __restrict__ output,
        uint32_t in_stride,
        uint32_t gate_offset,
        uint32_t n,
        uint32_t total) {
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx >= total) {
        return;
    }
    const uint32_t row = idx / n;
    const uint32_t inner = idx - row * n;
    const size_t base = static_cast<size_t>(row) * in_stride + gate_offset;
    const float gate = input[base + inner];
    const float value = input[base + n + inner];
    const float s = gate / (1.0f + expf(-gate));
    output[idx] = value * s;
}

extern "C" cudaError_t makepad_cuda_swiglu_gate_first_strided_f32(
        const float * input,
        float * output,
        uint32_t rows,
        uint32_t in_stride,
        uint32_t gate_offset,
        uint32_t n,
        cudaStream_t stream) {
    if (gate_offset + 2u * n > in_stride && rows > 0) {
        return cudaErrorInvalidValue;
    }
    const uint64_t total64 = static_cast<uint64_t>(rows) * n;
    if (total64 == 0 || total64 > 0xffffffffull) {
        return total64 == 0 ? cudaSuccess : cudaErrorInvalidValue;
    }
    const uint32_t total = static_cast<uint32_t>(total64);
    const dim3 block(256, 1, 1);
    const dim3 grid((total + block.x - 1) / block.x, 1, 1);
    makepad_cuda_swiglu_gate_first_strided_f32_kernel<<<grid, block, 0, stream>>>(
        input, output, in_stride, gate_offset, n, total);
    return cudaGetLastError();
}

// f16-input variant of the value-first SwiGLU (f32 output for the down gemm's
// bf16 convert path, or fed straight back as f16 via the f32->f16 pass).
static __global__ void makepad_cuda_swiglu_value_gate_f16_kernel(
        const __half * __restrict__ input,
        __half * __restrict__ output,
        uint32_t n,
        uint32_t total) {
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx >= total) {
        return;
    }
    const uint32_t row = idx / n;
    const uint32_t inner = idx - row * n;
    const size_t base = static_cast<size_t>(row) * (2u * n);
    const float value = __half2float(input[base + inner]);
    const float gate = __half2float(input[base + n + inner]);
    const float s = gate / (1.0f + expf(-gate));
    output[idx] = __float2half(value * s);
}

extern "C" cudaError_t makepad_cuda_swiglu_value_gate_f16(
        const uint16_t * input,
        uint16_t * output,
        uint32_t rows,
        uint32_t n,
        cudaStream_t stream) {
    const uint64_t total64 = static_cast<uint64_t>(rows) * n;
    if (total64 == 0 || total64 > 0xffffffffull) {
        return total64 == 0 ? cudaSuccess : cudaErrorInvalidValue;
    }
    const uint32_t total = static_cast<uint32_t>(total64);
    const dim3 block(256, 1, 1);
    const dim3 grid((total + block.x - 1) / block.x, 1, 1);
    makepad_cuda_swiglu_value_gate_f16_kernel<<<grid, block, 0, stream>>>(
        reinterpret_cast<const __half *>(input),
        reinterpret_cast<__half *>(output), n, total);
    return cudaGetLastError();
}

// Each kernel fuses an exact multi-launch recipe from the ACE DiT hot loop
// without changing any arithmetic: the same f32 operations in the same order,
// with __float2bfloat16_rn applied exactly where the separate bf16_round
// passes sat. One launch and one memory pass instead of two to four.

// rms_norm_rows_weighted_f32_f32weights_precise + bf16_round. Identical
// strided per-thread accumulation and block reduction (launched with the
// same block-size rule), round moved onto the store.
static __global__ void makepad_cuda_rms_norm_weighted_precise_round_bf16_kernel(
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
    sum = makepad_cuda_diff_block_reduce_sum(sum);
    __shared__ float inv_rms;
    if (threadIdx.x == 0) {
        inv_rms = rsqrtf(sum / static_cast<float>(n) + eps);
    }
    __syncthreads();
    for (uint32_t idx = threadIdx.x; idx < n; idx += blockDim.x) {
        row_out[idx] = __bfloat162float(__float2bfloat16_rn(
            row_in[idx] * inv_rms * weights_f32[idx]));
    }
}

extern "C" cudaError_t makepad_cuda_rms_norm_weighted_precise_round_bf16(
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
    makepad_cuda_rms_norm_weighted_precise_round_bf16_kernel<<<row_count, block, 0, stream>>>(
        input, weights_f32, output, row_count, row_stride, n, eps);
    return cudaGetLastError();
}

// ACE AdaLN single-store chain fused: the slice + add-ones + two
// gated_residual_vec launches become
//   out = fmaf(shift[c], 1.0f, fmaf(1.0f + scale[c], normed, 0.0f))
// (1.0f + scale matches add_f32_precise; both fmafs match
// gated_residual_vec against a +0 residual / a ones update).
static __global__ void makepad_cuda_adaln_mod_f32_kernel(
        const float * __restrict__ normed,
        const float * __restrict__ mods_scale,
        const float * __restrict__ mods_shift,
        float * __restrict__ output,
        uint32_t row_count,
        uint32_t cols) {
    const size_t total = static_cast<size_t>(row_count) * cols;
    const size_t idx = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    if (idx < total) {
        const uint32_t c = idx % cols;
        const float scale1 = 1.0f + mods_scale[c];
        const float scaled = fmaf(scale1, normed[idx], 0.0f);
        output[idx] = fmaf(mods_shift[c], 1.0f, scaled);
    }
}

extern "C" cudaError_t makepad_cuda_adaln_mod_f32(
        const float * normed,
        const float * mods_scale,
        const float * mods_shift,
        float * output,
        uint32_t row_count,
        uint32_t cols,
        cudaStream_t stream) {
    const size_t total = static_cast<size_t>(row_count) * cols;
    if (total == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<unsigned int>((total + block.x - 1) / block.x), 1, 1);
    makepad_cuda_adaln_mod_f32_kernel<<<grid, block, 0, stream>>>(
        normed, mods_scale, mods_shift, output, row_count, cols);
    return cudaGetLastError();
}

// rope_half_f32 + bf16_round: identical rotation expressions, round moved
// onto the stores (the separate round pass rounded the same values,
// pass-through channels included).
static __global__ void makepad_cuda_rope_half_round_bf16_f32_kernel(
        const float * __restrict__ input,
        const float * __restrict__ cos_table,
        const float * __restrict__ sin_table,
        float * __restrict__ output,
        uint32_t token_count,
        uint32_t head_count,
        uint32_t head_dim,
        uint32_t rot_half) {
    const uint32_t token = blockIdx.x;
    const uint32_t head = blockIdx.y;
    if (token >= token_count || head >= head_count) {
        return;
    }
    const size_t base = (static_cast<size_t>(token) * head_count + head)
        * static_cast<size_t>(head_dim);
    const size_t table_base = static_cast<size_t>(token) * rot_half;
    for (uint32_t i = threadIdx.x; i < rot_half; i += blockDim.x) {
        const float cos_v = cos_table[table_base + i];
        const float sin_v = sin_table[table_base + i];
        const float x1 = input[base + i];
        const float x2 = input[base + rot_half + i];
        output[base + i] = __bfloat162float(__float2bfloat16_rn(
            x1 * cos_v - x2 * sin_v));
        output[base + rot_half + i] = __bfloat162float(__float2bfloat16_rn(
            x2 * cos_v + x1 * sin_v));
    }
    for (uint32_t i = 2 * rot_half + threadIdx.x; i < head_dim; i += blockDim.x) {
        output[base + i] = __bfloat162float(__float2bfloat16_rn(input[base + i]));
    }
}

extern "C" cudaError_t makepad_cuda_rope_half_round_bf16_f32(
        const float * input,
        const float * cos_table,
        const float * sin_table,
        float * output,
        uint32_t token_count,
        uint32_t head_count,
        uint32_t head_dim,
        uint32_t rot_half,
        cudaStream_t stream) {
    if (token_count == 0 || head_count == 0 || rot_half == 0) {
        return cudaSuccess;
    }
    const dim3 block(rot_half < 128 ? 64 : 128, 1, 1);
    const dim3 grid(token_count, head_count, 1);
    makepad_cuda_rope_half_round_bf16_f32_kernel<<<grid, block, 0, stream>>>(
        input, cos_table, sin_table, output, token_count, head_count, head_dim, rot_half);
    return cudaGetLastError();
}

// gated_residual_vec(zeros, update, gate) + bf16_round + add_bf16:
//   out = round_bf16(h + round_bf16(fmaf(gate[c], update, 0.0f)))
// (+0.0f literal matches the +0.0 read from the zeros tensor).
static __global__ void makepad_cuda_gated_residual_round_add_bf16_f32_kernel(
        const float * __restrict__ h,
        const float * __restrict__ update,
        const float * __restrict__ gate,
        float * __restrict__ output,
        uint32_t row_count,
        uint32_t cols) {
    const size_t total = static_cast<size_t>(row_count) * cols;
    const size_t idx = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    if (idx < total) {
        const float gated = fmaf(gate[idx % cols], update[idx], 0.0f);
        const float rounded = __bfloat162float(__float2bfloat16_rn(gated));
        output[idx] = __bfloat162float(__float2bfloat16_rn(h[idx] + rounded));
    }
}

extern "C" cudaError_t makepad_cuda_gated_residual_round_add_bf16_f32(
        const float * h,
        const float * update,
        const float * gate,
        float * output,
        uint32_t row_count,
        uint32_t cols,
        cudaStream_t stream) {
    const size_t total = static_cast<size_t>(row_count) * cols;
    if (total == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<unsigned int>((total + block.x - 1) / block.x), 1, 1);
    makepad_cuda_gated_residual_round_add_bf16_f32_kernel<<<grid, block, 0, stream>>>(
        h, update, gate, output, row_count, cols);
    return cudaGetLastError();
}

// f32 -> bf16 words in one pass with round-to-nearest-even: bit-identical to
// the two-kernel staging recipe (bf16_round_f32 into f32 storage, then the
// truncating f32_to_bf16 — truncating an exactly-representable bf16 value is
// exact, so round-then-truncate == direct rn conversion).
static __global__ void makepad_cuda_f32_to_bf16_rn_kernel(
        const float * __restrict__ input,
        uint16_t * __restrict__ output,
        uint32_t n) {
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx < n) {
        const __nv_bfloat16 h = __float2bfloat16_rn(input[idx]);
        output[idx] = *reinterpret_cast<const uint16_t *>(&h);
    }
}

// f32 -> bf16-RN -> f16 carrier: the value set of the oracle's bf16
// operands, stored in the f16 encoding the FA2 f16 kernel consumes. Exact
// for |x| in [2^-14, 65504] (bf16 mantissa 7 <= f16 mantissa 10); flux2
// q/k post-RMS/rope and v activations sit well inside that range.
static __global__ void makepad_cuda_f32_to_bf16_rn_f16_kernel(
        const float * __restrict__ input,
        __half * __restrict__ output,
        uint32_t n) {
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx < n) {
        output[idx] = __float2half(__bfloat162float(__float2bfloat16_rn(input[idx])));
    }
}

extern "C" cudaError_t makepad_cuda_f32_to_bf16_rn_f16(
        const float * input,
        uint16_t * output,
        uint32_t n,
        cudaStream_t stream) {
    if (n == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid((n + block.x - 1) / block.x, 1, 1);
    makepad_cuda_f32_to_bf16_rn_f16_kernel<<<grid, block, 0, stream>>>(
        input, reinterpret_cast<__half *>(output), n);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_cuda_f32_to_bf16_rn(
        const float * input,
        uint16_t * output,
        uint32_t n,
        cudaStream_t stream) {
    if (n == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid((n + block.x - 1) / block.x, 1, 1);
    makepad_cuda_f32_to_bf16_rn_kernel<<<grid, block, 0, stream>>>(input, output, n);
    return cudaGetLastError();
}

// silu_f32_precise + bf16_round + mul_f32_precise + bf16_round (the ACE
// SwiGLU chain with PyTorch's bf16 boundary after both the activation and
// the product).
static __global__ void makepad_cuda_silu_round_mul_round_bf16_f32_kernel(
        const float * __restrict__ gate,
        const float * __restrict__ up,
        float * __restrict__ output,
        uint32_t n) {
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx < n) {
        const float x = gate[idx];
        const float s = x / (1.0f + expf(-x));
        const float sr = __bfloat162float(__float2bfloat16_rn(s));
        output[idx] = __bfloat162float(__float2bfloat16_rn(sr * up[idx]));
    }
}

extern "C" cudaError_t makepad_cuda_silu_round_mul_round_bf16_f32(
        const float * gate,
        const float * up,
        float * output,
        uint32_t n,
        cudaStream_t stream) {
    if (n == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid((n + block.x - 1) / block.x, 1, 1);
    makepad_cuda_silu_round_mul_round_bf16_f32_kernel<<<grid, block, 0, stream>>>(
        gate, up, output, n);
    return cudaGetLastError();
}

// --- IndexTTS kernels -------------------------------------------------------
// WaveNet cond gate on row-major (rows, 2n) data, biases already applied by
// the producing GEMM: out[row, c] = tanh(x[row, c]) * sigmoid(x[row, n + c]).
static __global__ void makepad_cuda_wavenet_gate_f32_kernel(
        const float * __restrict__ input,
        float * __restrict__ output,
        uint32_t n,
        uint32_t total) {
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx >= total) {
        return;
    }
    const uint32_t row = idx / n;
    const uint32_t inner = idx - row * n;
    const size_t base = static_cast<size_t>(row) * (2u * n);
    const float a = tanhf(input[base + inner]);
    const float g = input[base + n + inner];
    const float s = 1.0f / (1.0f + expf(-g));
    output[idx] = a * s;
}

extern "C" cudaError_t makepad_cuda_wavenet_gate_f32(
        const float * input,
        float * output,
        uint32_t rows,
        uint32_t n,
        cudaStream_t stream) {
    const uint64_t total64 = static_cast<uint64_t>(rows) * n;
    if (total64 == 0 || total64 > 0xffffffffull) {
        return total64 == 0 ? cudaSuccess : cudaErrorInvalidValue;
    }
    const uint32_t total = static_cast<uint32_t>(total64);
    const dim3 block(256, 1, 1);
    const dim3 grid((total + block.x - 1) / block.x, 1, 1);
    makepad_cuda_wavenet_gate_f32_kernel<<<grid, block, 0, stream>>>(
        input, output, n, total);
    return cudaGetLastError();
}

// Fused anti-aliased SnakeBeta (BigVGAN Activation1d, ratio 2, 12-tap Kaiser
// filters) on time-major (t, ch) rows, mirroring the CPU AliasFreeSnake
// exactly: replicate-pad 5 -> depthwise transposed conv stride 2 (12 taps,
// ratio gain prefolded) -> crop 15 -> x + sin(alpha*x)^2 * inv_beta ->
// replicate-pad 5/6 -> depthwise stride-2 conv (12 taps). Replicate padding
// is index clamping on both resamples. params is the combined per-activation
// buffer [alpha(ch) | inv_beta(ch) | up_filter(12) | down_filter(12)] with
// alpha/inv_beta preexponentiated on the host (no exp here). input_scale
// multiplies every input sample as loaded — before the up conv and therefore
// before the snake — carrying the upstream mean-of-blocks fold.
//
// Gather form of the transposed conv: crop position u sits at j = u + 15 in
// the scatter frame, fed by the 6 taps kk = p, p+2, .., p+10 of parity
// p = (u + 15) & 1 from input rows (j - kk) / 2 - 5 (clamped); the inner
// loop runs input-ascending to reproduce the CPU accumulation order.
static __global__ void makepad_cuda_alias_snake_updown2x_f32_kernel(
        const float * __restrict__ input,
        const float * __restrict__ params,
        float * __restrict__ output,
        uint32_t t_in,
        uint32_t ch,
        float input_scale,
        uint32_t total) {
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx >= total) {
        return;
    }
    const uint32_t t_out = idx / ch;
    const uint32_t c = idx - t_out * ch;
    const float alpha = params[c];
    const float inv_beta = params[ch + c];
    const float * up_f = params + 2u * ch;
    const float * down_f = up_f + 12;
    const int t_last = static_cast<int>(t_in) - 1;
    const int u_last = 2 * static_cast<int>(t_in) - 1;
    float sum = 0.0f;
    for (int kk = 0; kk < 12; kk++) {
        int u = 2 * static_cast<int>(t_out) + kk - 5;
        u = min(max(u, 0), u_last);
        const int p = 1 - (u & 1);
        const int m = (u + 15 - p) >> 1;
        float v = 0.0f;
        for (int i = 5; i >= 0; i--) {
            int xi = m - i - 5;
            xi = min(max(xi, 0), t_last);
            const float xv = input_scale * input[static_cast<size_t>(xi) * ch + c];
            v += up_f[p + 2 * i] * xv;
        }
        const float s = sinf(alpha * v);
        v += inv_beta * s * s;
        sum += down_f[kk] * v;
    }
    output[idx] = sum;
}

extern "C" cudaError_t makepad_cuda_alias_snake_updown2x_f32(
        const float * input,
        const float * params,
        float * output,
        uint32_t t_in,
        uint32_t ch,
        float input_scale,
        cudaStream_t stream) {
    const uint64_t total64 = static_cast<uint64_t>(t_in) * ch;
    if (total64 == 0 || total64 > 0xffffffffull ||
            static_cast<uint64_t>(t_in) > 0x3fffffffull) {
        return total64 == 0 ? cudaSuccess : cudaErrorInvalidValue;
    }
    const uint32_t total = static_cast<uint32_t>(total64);
    const dim3 block(256, 1, 1);
    const dim3 grid((total + block.x - 1) / block.x, 1, 1);
    makepad_cuda_alias_snake_updown2x_f32_kernel<<<grid, block, 0, stream>>>(
        input, params, output, t_in, ch, input_scale, total);
    return cudaGetLastError();
}

// --- TRELLIS 2 kernels ------------------------------------------------------
// Per-head weighted RMS norm: rows are (token*head) groups of width n, the
// weight vector spans heads*n and is selected by (row % heads)*n + idx —
// the TRELLIS MultiHeadRMSNorm (F.normalize * sqrt(n) * gamma == rms * gamma)
// with a DISTINCT gamma per head.
static __global__ void makepad_cuda_rms_norm_perhead_f32_kernel(
        const float * __restrict__ input,
        const float * __restrict__ weights,
        float * __restrict__ output,
        uint32_t row_count,
        uint32_t n,
        uint32_t heads,
        float eps) {
    const uint32_t row = blockIdx.x;
    if (row >= row_count) {
        return;
    }
    const float * row_in = input + static_cast<size_t>(row) * n;
    float * row_out = output + static_cast<size_t>(row) * n;
    const float * w = weights + static_cast<size_t>(row % heads) * n;
    float sum = 0.0f;
    for (uint32_t idx = threadIdx.x; idx < n; idx += blockDim.x) {
        const float v = row_in[idx];
        sum += v * v;
    }
    sum = makepad_cuda_diff_block_reduce_sum(sum);
    __shared__ float inv_rms;
    if (threadIdx.x == 0) {
        inv_rms = rsqrtf(sum / static_cast<float>(n) + eps);
    }
    __syncthreads();
    for (uint32_t idx = threadIdx.x; idx < n; idx += blockDim.x) {
        row_out[idx] = row_in[idx] * inv_rms * w[idx];
    }
}

extern "C" cudaError_t makepad_cuda_rms_norm_perhead_f32(
        const float * input,
        const float * weights,
        float * output,
        uint32_t row_count,
        uint32_t n,
        uint32_t heads,
        float eps,
        cudaStream_t stream) {
    if (row_count == 0 || n == 0 || heads == 0) {
        return cudaSuccess;
    }
    const dim3 block(n < 128 ? 32 : 128, 1, 1);
    const dim3 grid(row_count, 1, 1);
    makepad_cuda_rms_norm_perhead_f32_kernel<<<grid, block, 0, stream>>>(
        input, weights, output, row_count, n, heads, eps);
    return cudaGetLastError();
}

// Exact (erf) GELU — the DINOv3 MLP activation. gpu_gelu is the tanh
// approximation used by the DiT FFNs; DINOv3 needs the erf form.
static __global__ void makepad_cuda_gelu_erf_f32_kernel(
        const float * __restrict__ input,
        float * __restrict__ output,
        uint32_t total) {
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx >= total) {
        return;
    }
    const float x = input[idx];
    output[idx] = 0.5f * x * (1.0f + erff(x * 0.70710678118654752440f));
}

extern "C" cudaError_t makepad_cuda_gelu_erf_f32(
        const float * input,
        float * output,
        uint32_t total,
        cudaStream_t stream) {
    if (total == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid((total + block.x - 1) / block.x, 1, 1);
    makepad_cuda_gelu_erf_f32_kernel<<<grid, block, 0, stream>>>(input, output, total);
    return cudaGetLastError();
}

// General row gather with optional per-row column-block select:
//   out[i][0..block_cols] = src[row_idx[i]][colblock_idx[i]*block_cols ..]
// row_idx == 0xffffffff writes zeros (out-of-grid conv neighbors). With
// colblock_idx == nullptr the block index is 0 (plain row gather when
// block_cols == src row width). Composes the TRELLIS dense conv3d (27
// neighbor gathers), pixel-shuffle upsampling and the sparse C2S/S2C moves.
static __global__ void makepad_cuda_gather_rows_colblock_f32_kernel(
        const float * __restrict__ src,
        const uint32_t * __restrict__ row_idx,
        const uint32_t * __restrict__ colblock_idx,
        float * __restrict__ output,
        uint32_t out_rows,
        uint32_t src_row_stride,
        uint32_t block_cols) {
    const uint32_t row = blockIdx.x;
    if (row >= out_rows) {
        return;
    }
    float * out_row = output + static_cast<size_t>(row) * block_cols;
    const uint32_t src_row = row_idx[row];
    if (src_row == 0xffffffffu) {
        for (uint32_t idx = threadIdx.x; idx < block_cols; idx += blockDim.x) {
            out_row[idx] = 0.0f;
        }
        return;
    }
    const uint32_t block = colblock_idx == nullptr ? 0u : colblock_idx[row];
    const float * src_row_ptr = src + static_cast<size_t>(src_row) * src_row_stride
        + static_cast<size_t>(block) * block_cols;
    for (uint32_t idx = threadIdx.x; idx < block_cols; idx += blockDim.x) {
        out_row[idx] = src_row_ptr[idx];
    }
}

extern "C" cudaError_t makepad_cuda_gather_rows_colblock_f32(
        const float * src,
        const uint32_t * row_idx,
        const uint32_t * colblock_idx,
        float * output,
        uint32_t out_rows,
        uint32_t src_row_stride,
        uint32_t block_cols,
        cudaStream_t stream) {
    if (out_rows == 0 || block_cols == 0) {
        return cudaSuccess;
    }
    const dim3 block(block_cols < 128 ? 32 : 128, 1, 1);
    const dim3 grid(out_rows, 1, 1);
    makepad_cuda_gather_rows_colblock_f32_kernel<<<grid, block, 0, stream>>>(
        src, row_idx, colblock_idx, output, out_rows, src_row_stride, block_cols);
    return cudaGetLastError();
}

// Column gather shared across every row: out[r][j] = src[r][col_idx[j]].
// On planar [channel][y*w+x] tensors one index table re-addresses every
// channel at once, which composes the H3 VAE encoder's reflect padding
// (indices = reflected plane positions), the valid-region crop after a
// pad-0 conv, and the stride-2 subsample of a stride-1 conv output.
static __global__ void makepad_cuda_gather_cols_f32_kernel(
        const float * __restrict__ src,
        const uint32_t * __restrict__ col_idx,
        float * __restrict__ output,
        uint32_t rows,
        uint32_t src_cols,
        uint32_t out_cols) {
    const uint32_t col = blockIdx.x * blockDim.x + threadIdx.x;
    const uint32_t row = blockIdx.y;
    if (col >= out_cols || row >= rows) {
        return;
    }
    output[static_cast<size_t>(row) * out_cols + col] =
        src[static_cast<size_t>(row) * src_cols + col_idx[col]];
}

extern "C" cudaError_t makepad_cuda_gather_cols_f32(
        const float * src,
        const uint32_t * col_idx,
        float * output,
        uint32_t rows,
        uint32_t src_cols,
        uint32_t out_cols,
        cudaStream_t stream) {
    if (rows == 0 || out_cols == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid((out_cols + block.x - 1) / block.x, rows, 1);
    makepad_cuda_gather_cols_f32_kernel<<<grid, block, 0, stream>>>(
        src, col_idx, output, rows, src_cols, out_cols);
    return cudaGetLastError();
}

// im2col slab for the submanifold 3^3 sparse conv: slab row r (voxel row0+r)
// = the 27 neighbor feature rows concatenated tap-major (cols t*ci..t*ci+ci),
// converted to f16 for the single tensor-core gemm against the checkpoint's
// already-[Co, 27*Ci] flex_gemm weight. Absent neighbors (idx u32::MAX) are
// zero. `neighbors` is tap-major: neighbors[t * n_total + voxel].
static __global__ void makepad_cuda_gather27_f16_kernel(
        const float * __restrict__ src,
        const uint32_t * __restrict__ neighbors,
        __half * __restrict__ out,
        uint32_t row0,
        uint32_t rows,
        uint32_t n_total,
        uint32_t ci) {
    const uint32_t row = blockIdx.x;
    if (row >= rows) {
        return;
    }
    // One block per slab row: its 27 neighbor ids land in shared once, then
    // the threads stripe the whole 27*ci-column row.
    __shared__ uint32_t nbr[27];
    if (threadIdx.x < 27) {
        nbr[threadIdx.x] =
            neighbors[static_cast<size_t>(threadIdx.x) * n_total + row0 + row];
    }
    __syncthreads();
    const uint32_t k_total = 27u * ci;
    __half * dst = out + static_cast<size_t>(row) * k_total;
    for (uint32_t idx = threadIdx.x; idx < k_total; idx += blockDim.x) {
        const uint32_t tap = idx / ci;
        const uint32_t c = idx - tap * ci;
        const uint32_t src_row = nbr[tap];
        dst[idx] = src_row == 0xffffffffu
            ? __float2half_rn(0.0f)
            : __float2half_rn(src[static_cast<size_t>(src_row) * ci + c]);
    }
}

extern "C" cudaError_t makepad_cuda_gather27_f16(
        const float * src,
        const uint32_t * neighbors,
        uint16_t * out,
        uint32_t row0,
        uint32_t rows,
        uint32_t n_total,
        uint32_t ci,
        cudaStream_t stream) {
    if (rows == 0 || ci == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid(rows, 1, 1);
    makepad_cuda_gather27_f16_kernel<<<grid, block, 0, stream>>>(
        src, neighbors, reinterpret_cast<__half *>(out),
        row0, rows, n_total, ci);
    return cudaGetLastError();
}

// ---------------------------------------------------------------------------
// SA3 (Stable Audio 3) ops.
// ---------------------------------------------------------------------------

// DynamicTanh norm over groups of `width` values:
// out = tanh(alpha * x) * gamma + beta. gamma/beta are `width` long,
// alpha is a scalar. Used by the SA3 SAME-S autoencoder (block norms width
// 768, per-head qk norms width 64).
static __global__ void makepad_cuda_dyt_f32_kernel(
        const float * __restrict__ input,
        const float * __restrict__ gamma,
        const float * __restrict__ beta,
        float * __restrict__ output,
        uint32_t group_rows,
        uint32_t width,
        float alpha) {
    const uint32_t row = blockIdx.x;
    if (row >= group_rows) {
        return;
    }
    const size_t base = static_cast<size_t>(row) * width;
    for (uint32_t i = threadIdx.x; i < width; i += blockDim.x) {
        const float v = tanhf(alpha * input[base + i]);
        output[base + i] = v * gamma[i] + beta[i];
    }
}

extern "C" cudaError_t makepad_cuda_dyt_f32(
        const float * input,
        const float * gamma,
        const float * beta,
        float * output,
        uint32_t group_rows,
        uint32_t width,
        float alpha,
        cudaStream_t stream) {
    if (group_rows == 0 || width == 0) {
        return cudaSuccess;
    }
    const dim3 block(width < 128 ? 32 : 128, 1, 1);
    const dim3 grid(group_rows, 1, 1);
    makepad_cuda_dyt_f32_kernel<<<grid, block, 0, stream>>>(
        input, gamma, beta, output, group_rows, width, alpha);
    return cudaGetLastError();
}

// In-place attention-score post-pass for the SA3 T5Gemma encoder:
// scores = softcap * tanh(scores / softcap) (+ key_mask[col] when given;
// mask is 0 for valid keys, -inf for padded keys). Rows = heads * q_tokens,
// cols = kv_tokens. The qk gemm already applied the 1/sqrt(d) scale.
static __global__ void makepad_cuda_softcap_addmask_f32_kernel(
        float * __restrict__ scores,
        const float * __restrict__ key_mask,
        uint32_t rows,
        uint32_t cols,
        float softcap) {
    const uint32_t row = blockIdx.x;
    if (row >= rows) {
        return;
    }
    const size_t base = static_cast<size_t>(row) * cols;
    for (uint32_t i = threadIdx.x; i < cols; i += blockDim.x) {
        float v = softcap * tanhf(scores[base + i] / softcap);
        if (key_mask != nullptr) {
            v += key_mask[i];
        }
        scores[base + i] = v;
    }
}

extern "C" cudaError_t makepad_cuda_softcap_addmask_f32(
        float * scores,
        const float * key_mask,
        uint32_t rows,
        uint32_t cols,
        float softcap,
        cudaStream_t stream) {
    if (rows == 0 || cols == 0) {
        return cudaSuccess;
    }
    const dim3 block(cols < 128 ? 32 : 128, 1, 1);
    const dim3 grid(rows, 1, 1);
    makepad_cuda_softcap_addmask_f32_kernel<<<grid, block, 0, stream>>>(
        scores, key_mask, rows, cols, softcap);
    return cudaGetLastError();
}

// GeGLU with tanh-approximated gelu (T5Gemma MLP): rows hold
// [value(n), gate(n)]; out = value * gelu_tanh(gate).
static __global__ void makepad_cuda_geglu_tanh_value_gate_f32_kernel(
        const float * __restrict__ input,
        float * __restrict__ output,
        uint32_t n,
        uint32_t total) {
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx >= total) {
        return;
    }
    const uint32_t row = idx / n;
    const uint32_t inner = idx - row * n;
    const size_t base = static_cast<size_t>(row) * (2u * n);
    const float value = input[base + inner];
    const float g = input[base + n + inner];
    const float g3 = g * g * g;
    const float gelu = 0.5f * g * (1.0f + tanhf(0.7978845608f * (g + 0.044715f * g3)));
    output[idx] = value * gelu;
}

extern "C" cudaError_t makepad_cuda_geglu_tanh_value_gate_f32(
        const float * input,
        float * output,
        uint32_t rows,
        uint32_t n,
        cudaStream_t stream) {
    const uint32_t total = rows * n;
    if (total == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid((total + block.x - 1) / block.x, 1, 1);
    makepad_cuda_geglu_tanh_value_gate_f32_kernel<<<grid, block, 0, stream>>>(
        input, output, n, total);
    return cudaGetLastError();
}

// ---------------------------------------------------------------------------
// MOSS DAC snake activation (ADDITIVE — moss audio port; flag for dedup):
// out = x + (1/(alpha+1e-9)) * sin^2(alpha * x), alpha per COLUMN (the DAC
// device path keeps planes time-major: rows = samples, cols = channels).
// ---------------------------------------------------------------------------

static __global__ void makepad_cuda_snake_cols_f32_kernel(
        const float * __restrict__ input,
        const float * __restrict__ alpha,
        float * __restrict__ output,
        uint32_t row_count,
        uint32_t cols) {
    const size_t total = static_cast<size_t>(row_count) * cols;
    const size_t idx = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    if (idx < total) {
        const float a = alpha[idx % cols];
        const float x = input[idx];
        const float s = sinf(a * x);
        output[idx] = x + (1.0f / (a + 1e-9f)) * s * s;
    }
}

extern "C" cudaError_t makepad_cuda_snake_cols_f32(
        const float * input,
        const float * alpha,
        float * output,
        uint32_t row_count,
        uint32_t cols,
        cudaStream_t stream) {
    const size_t total = static_cast<size_t>(row_count) * cols;
    if (total == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<unsigned int>((total + block.x - 1) / block.x), 1, 1);
    makepad_cuda_snake_cols_f32_kernel<<<grid, block, 0, stream>>>(
        input, alpha, output, row_count, cols);
    return cudaGetLastError();
}

// ---------------------------------------------------------------------------
// SkinTokens Qwen beam decode. Cache rows are beam-major token-major:
// [beam][sequence][kv_head * head_dim]. The attention kernel maps each query
// head to its grouped KV head and keeps beam contexts independent without
// materializing repeated K/V heads or a B*H*S score tensor.
// ---------------------------------------------------------------------------

static __global__ void makepad_cuda_beam_cache_reorder_append_f32_kernel(
        const float * __restrict__ prior,
        const float * __restrict__ step,
        const uint32_t * __restrict__ parents,
        float * __restrict__ output,
        uint32_t prior_sequence,
        uint32_t output_beams,
        uint32_t cols) {
    const size_t output_rows = static_cast<size_t>(output_beams) * (prior_sequence + 1u);
    const size_t total = output_rows * cols;
    const size_t index = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    if (index >= total) {
        return;
    }
    const uint32_t col = static_cast<uint32_t>(index % cols);
    const size_t row = index / cols;
    const uint32_t beam = static_cast<uint32_t>(row / (prior_sequence + 1u));
    const uint32_t position = static_cast<uint32_t>(row - static_cast<size_t>(beam) * (prior_sequence + 1u));
    if (position < prior_sequence) {
        const size_t source_row = static_cast<size_t>(parents[beam]) * prior_sequence + position;
        output[index] = prior[source_row * cols + col];
    } else {
        output[index] = step[static_cast<size_t>(beam) * cols + col];
    }
}

extern "C" cudaError_t makepad_cuda_beam_cache_reorder_append_f32(
        const float * prior,
        const float * step,
        const uint32_t * parents,
        float * output,
        uint32_t prior_sequence,
        uint32_t output_beams,
        uint32_t cols,
        cudaStream_t stream) {
    const size_t total = static_cast<size_t>(output_beams) * (prior_sequence + 1u) * cols;
    if (total == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<unsigned int>((total + block.x - 1) / block.x), 1, 1);
    makepad_cuda_beam_cache_reorder_append_f32_kernel<<<grid, block, 0, stream>>>(
        prior, step, parents, output, prior_sequence, output_beams, cols);
    return cudaGetLastError();
}

static __global__ void makepad_cuda_attention_gqa_decode_bf16_f32_kernel(
        const float * __restrict__ query,
        const float * __restrict__ key,
        const float * __restrict__ value,
        float * __restrict__ output,
        uint32_t beams,
        uint32_t sequence,
        uint32_t query_heads,
        uint32_t kv_heads,
        uint32_t head_dim,
        float scale) {
    const uint32_t beam = blockIdx.x;
    const uint32_t query_head = blockIdx.y;
    const uint32_t dim = threadIdx.x;
    if (beam >= beams || query_head >= query_heads || dim >= head_dim) {
        return;
    }
    const uint32_t group = query_heads / kv_heads;
    const uint32_t kv_head = query_head / group;
    const uint32_t query_width = query_heads * head_dim;
    const uint32_t kv_width = kv_heads * head_dim;
    const float q = query[static_cast<size_t>(beam) * query_width
        + static_cast<size_t>(query_head) * head_dim + dim];
    __shared__ float maximum;
    __shared__ float denominator;
    __shared__ float probability;
    if (dim == 0) {
        maximum = -CUDART_INF_F;
        denominator = 0.0f;
    }
    __syncthreads();

    // Pass 1: row maximum. Each block is exactly one (beam, query head),
    // with one thread per head dimension.
    for (uint32_t position = 0; position < sequence; ++position) {
        const size_t k_index = (static_cast<size_t>(beam) * sequence + position) * kv_width
            + static_cast<size_t>(kv_head) * head_dim + dim;
        float dot = makepad_cuda_diff_block_reduce_sum(q * key[k_index]);
        if (dim == 0) {
            maximum = fmaxf(maximum, dot * scale);
        }
        __syncthreads();
    }

    // Pass 2: f32 softmax denominator.
    for (uint32_t position = 0; position < sequence; ++position) {
        const size_t k_index = (static_cast<size_t>(beam) * sequence + position) * kv_width
            + static_cast<size_t>(kv_head) * head_dim + dim;
        float dot = makepad_cuda_diff_block_reduce_sum(q * key[k_index]);
        if (dim == 0) {
            denominator += expf(dot * scale - maximum);
        }
        __syncthreads();
    }

    // Pass 3: PyTorch autocast rounds softmax probabilities to BF16 before
    // the probability/value tensor-core product, with f32 accumulation.
    float accumulator = 0.0f;
    for (uint32_t position = 0; position < sequence; ++position) {
        const size_t k_index = (static_cast<size_t>(beam) * sequence + position) * kv_width
            + static_cast<size_t>(kv_head) * head_dim + dim;
        float dot = makepad_cuda_diff_block_reduce_sum(q * key[k_index]);
        if (dim == 0) {
            const float p = expf(dot * scale - maximum) / denominator;
            probability = __bfloat162float(__float2bfloat16_rn(p));
        }
        __syncthreads();
        const size_t v_index = (static_cast<size_t>(beam) * sequence + position) * kv_width
            + static_cast<size_t>(kv_head) * head_dim + dim;
        accumulator += probability * value[v_index];
        __syncthreads();
    }
    output[static_cast<size_t>(beam) * query_width
        + static_cast<size_t>(query_head) * head_dim + dim] = accumulator;
}

extern "C" cudaError_t makepad_cuda_attention_gqa_decode_bf16_f32(
        const float * query,
        const float * key,
        const float * value,
        float * output,
        uint32_t beams,
        uint32_t sequence,
        uint32_t query_heads,
        uint32_t kv_heads,
        uint32_t head_dim,
        float scale,
        cudaStream_t stream) {
    if (beams == 0 || sequence == 0 || query_heads == 0 || kv_heads == 0
            || head_dim == 0 || query_heads % kv_heads != 0 || head_dim > 1024) {
        return (beams == 0 || sequence == 0) ? cudaSuccess : cudaErrorInvalidValue;
    }
    const dim3 block(head_dim, 1, 1);
    const dim3 grid(beams, query_heads, 1);
    makepad_cuda_attention_gqa_decode_bf16_f32_kernel<<<grid, block, 0, stream>>>(
        query, key, value, output, beams, sequence, query_heads, kv_heads, head_dim, scale);
    return cudaGetLastError();
}

// Pair (cond, uncond) GQA decode over two separate in-place KV caches.
// Byte-identical math to makepad_cuda_attention_gqa_decode_bf16_f32 on
// the row-concatenated caches, restructured for parallelism: the serial
// kernel above walks the sequence three times from beams*query_heads blocks
// (~64 on this model), which is the O(seq) wall in Music3 AR decode. Here
// pass 1 computes every (beam, head, position) dot in its own block with the
// SAME 128-thread block reduction (same bits), and pass 2 keeps the original
// order-sensitive serial softmax/accumulate loops per (beam, head) — those
// chains are short-latency scalar work, not block reductions.

static __global__ void makepad_cuda_gqa_decode_pair_dots_f32_kernel(
        const float * __restrict__ query,
        const float * __restrict__ key0,
        const float * __restrict__ key1,
        float * __restrict__ dots,
        uint32_t sequence,
        uint32_t query_heads,
        uint32_t kv_heads,
        uint32_t head_dim) {
    // One block per (beam, kv head, position); the K row segment is read
    // once and dotted against each grouped query head with the SAME block
    // reduction (and the same per-dot bits) as the serial kernel.
    const uint32_t position = blockIdx.x;
    const uint32_t kv_head = blockIdx.y;
    const uint32_t beam = blockIdx.z;
    const uint32_t dim = threadIdx.x;
    if (position >= sequence || kv_head >= kv_heads || dim >= head_dim) {
        return;
    }
    const uint32_t group = query_heads / kv_heads;
    const uint32_t query_width = query_heads * head_dim;
    const uint32_t kv_width = kv_heads * head_dim;
    const float * __restrict__ key = (beam == 0) ? key0 : key1;
    const float k = key[static_cast<size_t>(position) * kv_width
        + static_cast<size_t>(kv_head) * head_dim + dim];
    for (uint32_t g = 0; g < group; ++g) {
        const uint32_t query_head = kv_head * group + g;
        const float q = query[static_cast<size_t>(beam) * query_width
            + static_cast<size_t>(query_head) * head_dim + dim];
        float dot = makepad_cuda_diff_block_reduce_sum(q * k);
        if (dim == 0) {
            dots[(static_cast<size_t>(beam) * query_heads + query_head)
                * sequence + position] = dot;
        }
        // The block reduction reuses its shared scratch each iteration.
        __syncthreads();
    }
}

static __global__ void makepad_cuda_gqa_decode_pair_out_f32_kernel(
        const float * __restrict__ dots,
        const float * __restrict__ value0,
        const float * __restrict__ value1,
        float * __restrict__ output,
        uint32_t sequence,
        uint32_t query_heads,
        uint32_t kv_heads,
        uint32_t head_dim,
        float scale) {
    // Dynamic shared: `sequence` exp values, filled position-parallel.
    extern __shared__ float exp_shared[];
    const uint32_t query_head = blockIdx.x;
    const uint32_t beam = blockIdx.y;
    const uint32_t dim = threadIdx.x;
    if (query_head >= query_heads || dim >= head_dim) {
        return;
    }
    const uint32_t group = query_heads / kv_heads;
    const uint32_t kv_head = query_head / group;
    const uint32_t query_width = query_heads * head_dim;
    const uint32_t kv_width = kv_heads * head_dim;
    const float * __restrict__ drow = dots
        + (static_cast<size_t>(beam) * query_heads + query_head) * sequence;

    // Pass 1: row maximum, strided per thread then tree-reduced. fmaxf is
    // exact so the reduction order cannot change the value; the serial
    // kernel's O(seq) dependent-load chain (one load + fmaxf per position on
    // every thread) was pure latency.
    __shared__ float max_shared[1024];
    float local_max = -CUDART_INF_F;
    for (uint32_t position = dim; position < sequence; position += blockDim.x) {
        local_max = fmaxf(local_max, drow[position] * scale);
    }
    max_shared[dim] = local_max;
    __syncthreads();
    for (uint32_t stride = blockDim.x >> 1; stride > 0; stride >>= 1) {
        if (dim < stride && dim + stride < blockDim.x) {
            max_shared[dim] = fmaxf(max_shared[dim], max_shared[dim + stride]);
        }
        __syncthreads();
    }
    const float maximum = max_shared[0];

    // Pass 2a: exp terms, position-parallel. Each position's exp input and
    // therefore its bits are identical to the serial kernel's; only the
    // evaluation order changes, and exp terms have no cross-position
    // dependency. This removes the serial per-position global-load + expf
    // latency chain that dominated the old thread-0 loop.
    for (uint32_t position = dim; position < sequence; position += blockDim.x) {
        exp_shared[position] = expf(drow[position] * scale - maximum);
    }
    __syncthreads();

    // Pass 2b: f32 softmax denominator serially in the original position
    // order over the prestored exp terms — the exact bits and addition
    // order of the serial kernel's accumulation.
    __shared__ float denom_shared;
    if (dim == 0) {
        float denominator = 0.0f;
        for (uint32_t position = 0; position < sequence; ++position) {
            denominator += exp_shared[position];
        }
        denom_shared = denominator;
    }
    __syncthreads();
    const float denominator = denom_shared;

    // Pass 3: BF16-rounded probabilities times V, f32-accumulated serially in
    // position order, one thread per head dimension — same as the serial
    // kernel's per-thread accumulator (p = exp/denominator, identical bits).
    // The V loads are hand-pipelined 8 deep: this grid is only 2*query_heads
    // blocks (~1 warp per SM), so a plain loop stalls one full global-load
    // latency per position. Batching the loads keeps 8 in flight while the
    // single accumulator still adds strictly ascending positions.
    const float * __restrict__ value = (beam == 0) ? value0 : value1;
    const float * __restrict__ v_col = value
        + static_cast<size_t>(kv_head) * head_dim + dim;
    float accumulator = 0.0f;
    uint32_t position = 0;
    for (; position + 8 <= sequence; position += 8) {
        float v[8];
        #pragma unroll
        for (uint32_t j = 0; j < 8; ++j) {
            v[j] = v_col[static_cast<size_t>(position + j) * kv_width];
        }
        #pragma unroll
        for (uint32_t j = 0; j < 8; ++j) {
            const float p = exp_shared[position + j] / denominator;
            const float probability = __bfloat162float(__float2bfloat16_rn(p));
            accumulator += probability * v[j];
        }
    }
    for (; position < sequence; ++position) {
        const float p = exp_shared[position] / denominator;
        const float probability = __bfloat162float(__float2bfloat16_rn(p));
        accumulator += probability * v_col[static_cast<size_t>(position) * kv_width];
    }
    output[static_cast<size_t>(beam) * query_width
        + static_cast<size_t>(query_head) * head_dim + dim] = accumulator;
}

// Fallback out-kernel for sequences too long for the shared-exp variant:
// same serial order-sensitive loops, exp recomputed per pass (identical bits).
static __global__ void makepad_cuda_gqa_decode_pair_out_noshared_f32_kernel(
        const float * __restrict__ dots,
        const float * __restrict__ value0,
        const float * __restrict__ value1,
        float * __restrict__ output,
        uint32_t sequence,
        uint32_t query_heads,
        uint32_t kv_heads,
        uint32_t head_dim,
        float scale) {
    const uint32_t query_head = blockIdx.x;
    const uint32_t beam = blockIdx.y;
    const uint32_t dim = threadIdx.x;
    if (query_head >= query_heads || dim >= head_dim) {
        return;
    }
    const uint32_t group = query_heads / kv_heads;
    const uint32_t kv_head = query_head / group;
    const uint32_t query_width = query_heads * head_dim;
    const uint32_t kv_width = kv_heads * head_dim;
    const float * __restrict__ drow = dots
        + (static_cast<size_t>(beam) * query_heads + query_head) * sequence;
    float maximum = -CUDART_INF_F;
    for (uint32_t position = 0; position < sequence; ++position) {
        const float dot = drow[position];
        maximum = fmaxf(maximum, dot * scale);
    }
    float denominator = 0.0f;
    for (uint32_t position = 0; position < sequence; ++position) {
        const float dot = drow[position];
        denominator += expf(dot * scale - maximum);
    }
    const float * __restrict__ value = (beam == 0) ? value0 : value1;
    float accumulator = 0.0f;
    for (uint32_t position = 0; position < sequence; ++position) {
        const float dot = drow[position];
        const float p = expf(dot * scale - maximum) / denominator;
        const float probability = __bfloat162float(__float2bfloat16_rn(p));
        const size_t v_index = static_cast<size_t>(position) * kv_width
            + static_cast<size_t>(kv_head) * head_dim + dim;
        accumulator += probability * value[v_index];
    }
    output[static_cast<size_t>(beam) * query_width
        + static_cast<size_t>(query_head) * head_dim + dim] = accumulator;
}

extern "C" cudaError_t makepad_cuda_attention_gqa_decode_pair_bf16_f32(
        const float * query,
        const float * key0,
        const float * value0,
        const float * key1,
        const float * value1,
        float * dots,
        float * output,
        uint32_t sequence,
        uint32_t query_heads,
        uint32_t kv_heads,
        uint32_t head_dim,
        float scale,
        cudaStream_t stream) {
    if (sequence == 0 || query_heads == 0 || kv_heads == 0 || head_dim == 0
            || query_heads % kv_heads != 0 || head_dim > 1024) {
        return sequence == 0 ? cudaSuccess : cudaErrorInvalidValue;
    }
    const dim3 block(head_dim, 1, 1);
    const dim3 dots_grid(sequence, kv_heads, 2);
    makepad_cuda_gqa_decode_pair_dots_f32_kernel<<<dots_grid, block, 0, stream>>>(
        query, key0, key1, dots, sequence, query_heads, kv_heads, head_dim);
    cudaError_t err = cudaGetLastError();
    if (err != cudaSuccess) {
        return err;
    }
    const dim3 out_grid(query_heads, 2, 1);
    const size_t exp_bytes = static_cast<size_t>(sequence) * sizeof(float);
    if (exp_bytes <= 44000) {
        makepad_cuda_gqa_decode_pair_out_f32_kernel<<<out_grid, block, exp_bytes, stream>>>(
            dots, value0, value1, output, sequence, query_heads, kv_heads, head_dim, scale);
    } else {
        makepad_cuda_gqa_decode_pair_out_noshared_f32_kernel<<<out_grid, block, 0, stream>>>(
            dots, value0, value1, output, sequence, query_heads, kv_heads, head_dim, scale);
    }
    return cudaGetLastError();
}

// ---------------------------------------------------------------------------
// BiRefNet HR-matting primitives.  These stay deliberately layout-specific:
// transformer tensors are row-major [token, channel], while decoder feature
// maps are planar [channel, y*x].  Keeping both layouts explicit avoids the
// multi-gigabyte host round trips in the reference implementation.

static __global__ void makepad_cuda_birefnet_relu_f32_kernel(
        const float * __restrict__ input,
        float * __restrict__ output,
        size_t n) {
    const size_t i = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    if (i < n) {
        output[i] = fmaxf(input[i], 0.0f);
    }
}

extern "C" cudaError_t makepad_cuda_birefnet_relu_f32(
        const float * input,
        float * output,
        size_t n,
        cudaStream_t stream) {
    if (n == 0) return cudaSuccess;
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<unsigned int>((n + block.x - 1) / block.x), 1, 1);
    makepad_cuda_birefnet_relu_f32_kernel<<<grid, block, 0, stream>>>(input, output, n);
    return cudaGetLastError();
}

static __global__ void makepad_cuda_birefnet_resize_bilinear_f32_kernel(
        const float * __restrict__ input,
        float * __restrict__ output,
        uint32_t in_width,
        uint32_t in_height,
        uint32_t out_width,
        uint32_t out_height,
        uint32_t channels,
        uint32_t align_corners) {
    const size_t out_plane = static_cast<size_t>(out_width) * out_height;
    const size_t i = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    if (i >= out_plane * channels) return;
    const uint32_t channel = static_cast<uint32_t>(i / out_plane);
    const uint32_t p = static_cast<uint32_t>(i - static_cast<size_t>(channel) * out_plane);
    const uint32_t oy = p / out_width;
    const uint32_t ox = p - oy * out_width;
    float fy;
    float fx;
    if (align_corners) {
        fy = out_height > 1 ? static_cast<float>(oy) * static_cast<float>(in_height - 1)
            / static_cast<float>(out_height - 1) : 0.0f;
        fx = out_width > 1 ? static_cast<float>(ox) * static_cast<float>(in_width - 1)
            / static_cast<float>(out_width - 1) : 0.0f;
    } else {
        fy = (static_cast<float>(oy) + 0.5f) * static_cast<float>(in_height)
            / static_cast<float>(out_height) - 0.5f;
        fx = (static_cast<float>(ox) + 0.5f) * static_cast<float>(in_width)
            / static_cast<float>(out_width) - 0.5f;
        fy = fmaxf(fy, 0.0f);
        fx = fmaxf(fx, 0.0f);
    }
    const uint32_t y0 = min(static_cast<uint32_t>(floorf(fy)), in_height - 1);
    const uint32_t x0 = min(static_cast<uint32_t>(floorf(fx)), in_width - 1);
    const uint32_t y1 = min(y0 + 1, in_height - 1);
    const uint32_t x1 = min(x0 + 1, in_width - 1);
    const float ly = fy - static_cast<float>(y0);
    const float lx = fx - static_cast<float>(x0);
    const size_t in_plane = static_cast<size_t>(in_width) * in_height;
    const float * src = input + static_cast<size_t>(channel) * in_plane;
    const float top = src[static_cast<size_t>(y0) * in_width + x0] * (1.0f - lx)
        + src[static_cast<size_t>(y0) * in_width + x1] * lx;
    const float bottom = src[static_cast<size_t>(y1) * in_width + x0] * (1.0f - lx)
        + src[static_cast<size_t>(y1) * in_width + x1] * lx;
    output[i] = top * (1.0f - ly) + bottom * ly;
}

extern "C" cudaError_t makepad_cuda_birefnet_resize_bilinear_f32(
        const float * input,
        float * output,
        uint32_t in_width,
        uint32_t in_height,
        uint32_t out_width,
        uint32_t out_height,
        uint32_t channels,
        uint32_t align_corners,
        cudaStream_t stream) {
    if (!in_width || !in_height || !out_width || !out_height || !channels) {
        return cudaErrorInvalidValue;
    }
    const size_t n = static_cast<size_t>(out_width) * out_height * channels;
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<unsigned int>((n + block.x - 1) / block.x), 1, 1);
    makepad_cuda_birefnet_resize_bilinear_f32_kernel<<<grid, block, 0, stream>>>(
        input, output, in_width, in_height, out_width, out_height, channels, align_corners);
    return cudaGetLastError();
}

static __global__ void makepad_cuda_birefnet_tokens_to_planar_f32_kernel(
        const float * __restrict__ input,
        float * __restrict__ output,
        uint32_t tokens,
        uint32_t channels) {
    const size_t i = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    const size_t n = static_cast<size_t>(tokens) * channels;
    if (i < n) {
        const uint32_t token = static_cast<uint32_t>(i / channels);
        const uint32_t channel = static_cast<uint32_t>(i - static_cast<size_t>(token) * channels);
        output[static_cast<size_t>(channel) * tokens + token] = input[i];
    }
}

extern "C" cudaError_t makepad_cuda_birefnet_tokens_to_planar_f32(
        const float * input,
        float * output,
        uint32_t tokens,
        uint32_t channels,
        cudaStream_t stream) {
    const size_t n = static_cast<size_t>(tokens) * channels;
    if (n == 0) return cudaSuccess;
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<unsigned int>((n + block.x - 1) / block.x), 1, 1);
    makepad_cuda_birefnet_tokens_to_planar_f32_kernel<<<grid, block, 0, stream>>>(
        input, output, tokens, channels);
    return cudaGetLastError();
}

// Pixel shuffle used by stride==kernel ConvTranspose2d heads (DA3 DPT).
// Input is planar [out_channels * scale * scale][in_h][in_w], with the
// sub-pixel feature order [channel][ky][kx].  The operation has no overlap
// for this exact transposed-convolution contract, so it is a pure layout
// transform plus the per-output-channel bias.
static __global__ void makepad_cuda_pixel_shuffle_planar_f32_kernel(
        const float * __restrict__ input,
        const float * __restrict__ bias,
        float * __restrict__ output,
        uint32_t in_width,
        uint32_t in_height,
        uint32_t out_channels,
        uint32_t scale) {
    const uint32_t out_width = in_width * scale;
    const uint32_t out_height = in_height * scale;
    const size_t out_plane = static_cast<size_t>(out_width) * out_height;
    const size_t i = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    const size_t n = static_cast<size_t>(out_channels) * out_plane;
    if (i >= n) return;
    const uint32_t channel = static_cast<uint32_t>(i / out_plane);
    const uint32_t pixel = static_cast<uint32_t>(i - static_cast<size_t>(channel) * out_plane);
    const uint32_t oy = pixel / out_width;
    const uint32_t ox = pixel - oy * out_width;
    const uint32_t iy = oy / scale;
    const uint32_t ix = ox / scale;
    const uint32_t ky = oy - iy * scale;
    const uint32_t kx = ox - ix * scale;
    const uint32_t feature = (channel * scale + ky) * scale + kx;
    const size_t in_plane = static_cast<size_t>(in_width) * in_height;
    output[i] = input[static_cast<size_t>(feature) * in_plane
        + static_cast<size_t>(iy) * in_width + ix] + bias[channel];
}

extern "C" cudaError_t makepad_cuda_pixel_shuffle_planar_f32(
        const float * input,
        const float * bias,
        float * output,
        uint32_t in_width,
        uint32_t in_height,
        uint32_t out_channels,
        uint32_t scale,
        cudaStream_t stream) {
    if (!in_width || !in_height || !out_channels || !scale) {
        return cudaErrorInvalidValue;
    }
    const size_t n = static_cast<size_t>(out_channels)
        * in_width * scale * in_height * scale;
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<unsigned int>((n + block.x - 1) / block.x), 1, 1);
    makepad_cuda_pixel_shuffle_planar_f32_kernel<<<grid, block, 0, stream>>>(
        input, bias, output, in_width, in_height, out_channels, scale);
    return cudaGetLastError();
}

static __global__ void makepad_cuda_birefnet_image_to_patches_f32_kernel(
        const float * __restrict__ input,
        float * __restrict__ output,
        uint32_t image_width,
        uint32_t image_height,
        uint32_t out_width,
        uint32_t out_height,
        uint32_t channels) {
    const uint32_t hg = image_height / out_height;
    const uint32_t wg = image_width / out_width;
    const uint32_t out_channels = channels * hg * wg;
    const size_t out_plane = static_cast<size_t>(out_width) * out_height;
    const size_t i = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    if (i >= static_cast<size_t>(out_channels) * out_plane) return;
    const uint32_t oc = static_cast<uint32_t>(i / out_plane);
    const uint32_t p = static_cast<uint32_t>(i - static_cast<size_t>(oc) * out_plane);
    const uint32_t c = oc / (hg * wg);
    const uint32_t patch = oc - c * hg * wg;
    const uint32_t ih = patch / wg;
    const uint32_t iw = patch - ih * wg;
    const uint32_t y = p / out_width;
    const uint32_t x = p - y * out_width;
    const size_t input_plane = static_cast<size_t>(image_width) * image_height;
    output[i] = input[static_cast<size_t>(c) * input_plane
        + static_cast<size_t>(ih * out_height + y) * image_width
        + iw * out_width + x];
}

extern "C" cudaError_t makepad_cuda_birefnet_image_to_patches_f32(
        const float * input,
        float * output,
        uint32_t image_width,
        uint32_t image_height,
        uint32_t out_width,
        uint32_t out_height,
        uint32_t channels,
        cudaStream_t stream) {
    if (!image_width || !image_height || !out_width || !out_height || !channels
            || image_width % out_width || image_height % out_height) {
        return cudaErrorInvalidValue;
    }
    const size_t n = static_cast<size_t>(channels)
        * (image_height / out_height) * (image_width / out_width)
        * out_height * out_width;
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<unsigned int>((n + block.x - 1) / block.x), 1, 1);
    makepad_cuda_birefnet_image_to_patches_f32_kernel<<<grid, block, 0, stream>>>(
        input, output, image_width, image_height, out_width, out_height, channels);
    return cudaGetLastError();
}

static __global__ void makepad_cuda_birefnet_global_avg_pool_f32_kernel(
        const float * __restrict__ input,
        float * __restrict__ output,
        uint32_t plane) {
    const uint32_t channel = blockIdx.x;
    float sum = 0.0f;
    for (uint32_t p = threadIdx.x; p < plane; p += blockDim.x) {
        sum += input[static_cast<size_t>(channel) * plane + p];
    }
    sum = makepad_cuda_diff_block_reduce_sum(sum);
    if (threadIdx.x == 0) output[channel] = sum / static_cast<float>(plane);
}

extern "C" cudaError_t makepad_cuda_birefnet_global_avg_pool_f32(
        const float * input,
        float * output,
        uint32_t plane,
        uint32_t channels,
        cudaStream_t stream) {
    if (!plane || !channels) return cudaErrorInvalidValue;
    makepad_cuda_birefnet_global_avg_pool_f32_kernel<<<channels, 256, 0, stream>>>(
        input, output, plane);
    return cudaGetLastError();
}

static __global__ void makepad_cuda_birefnet_broadcast_f32_kernel(
        const float * __restrict__ input,
        float * __restrict__ output,
        uint32_t plane,
        size_t n) {
    const size_t i = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    if (i < n) output[i] = input[i / plane];
}

extern "C" cudaError_t makepad_cuda_birefnet_broadcast_f32(
        const float * input,
        float * output,
        uint32_t plane,
        uint32_t channels,
        cudaStream_t stream) {
    const size_t n = static_cast<size_t>(plane) * channels;
    if (!n) return cudaSuccess;
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<unsigned int>((n + block.x - 1) / block.x), 1, 1);
    makepad_cuda_birefnet_broadcast_f32_kernel<<<grid, block, 0, stream>>>(
        input, output, plane, n);
    return cudaGetLastError();
}

static __global__ void makepad_cuda_birefnet_mul_sigmoid_mask_f32_kernel(
        const float * __restrict__ input,
        const float * __restrict__ logits,
        float * __restrict__ output,
        uint32_t plane,
        size_t n) {
    const size_t i = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    if (i < n) {
        const float z = logits[i % plane];
        output[i] = input[i] / (1.0f + expf(-z));
    }
}

extern "C" cudaError_t makepad_cuda_birefnet_mul_sigmoid_mask_f32(
        const float * input,
        const float * logits,
        float * output,
        uint32_t plane,
        uint32_t channels,
        cudaStream_t stream) {
    const size_t n = static_cast<size_t>(plane) * channels;
    if (!n) return cudaSuccess;
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<unsigned int>((n + block.x - 1) / block.x), 1, 1);
    makepad_cuda_birefnet_mul_sigmoid_mask_f32_kernel<<<grid, block, 0, stream>>>(
        input, logits, output, plane, n);
    return cudaGetLastError();
}

__device__ __forceinline__ float makepad_cuda_birefnet_bilinear(
        const float * image, uint32_t height, uint32_t width, float y, float x) {
    if (y <= -1.0f || y >= static_cast<float>(height)
            || x <= -1.0f || x >= static_cast<float>(width)) return 0.0f;
    const int y0 = static_cast<int>(floorf(y));
    const int x0 = static_cast<int>(floorf(x));
    const int y1 = y0 + 1;
    const int x1 = x0 + 1;
    const float ly = y - static_cast<float>(y0);
    const float lx = x - static_cast<float>(x0);
    const float hy = 1.0f - ly;
    const float hx = 1.0f - lx;
    const float v00 = y0 >= 0 && x0 >= 0 ? image[static_cast<size_t>(y0) * width + x0] : 0.0f;
    const float v01 = y0 >= 0 && x1 < static_cast<int>(width)
        ? image[static_cast<size_t>(y0) * width + x1] : 0.0f;
    const float v10 = y1 < static_cast<int>(height) && x0 >= 0
        ? image[static_cast<size_t>(y1) * width + x0] : 0.0f;
    const float v11 = y1 < static_cast<int>(height) && x1 < static_cast<int>(width)
        ? image[static_cast<size_t>(y1) * width + x1] : 0.0f;
    return hy * hx * v00 + hy * lx * v01 + ly * hx * v10 + ly * lx * v11;
}

// Output is column-major [m_chunk, Cin*K*K], ready for one tensor-core GEMM.
static __global__ void makepad_cuda_birefnet_deform_im2col_f32_to_f16_kernel(
        const float * __restrict__ input,
        const float * __restrict__ offset,
        const float * __restrict__ modulator,
        __half * __restrict__ output,
        uint32_t width,
        uint32_t height,
        uint32_t kernel,
        uint32_t padding,
        uint32_t p0,
        uint32_t m_chunk) {
    const uint32_t col = blockIdx.y;
    const uint32_t row = blockIdx.x * blockDim.x + threadIdx.x;
    if (row >= m_chunk) return;
    const uint32_t kernel2 = kernel * kernel;
    const uint32_t tap = col % kernel2;
    const uint32_t channel = col / kernel2;
    const uint32_t ky = tap / kernel;
    const uint32_t kx = tap - ky * kernel;
    const uint32_t p = p0 + row;
    const uint32_t oy = p / width;
    const uint32_t ox = p - oy * width;
    const size_t plane = static_cast<size_t>(width) * height;
    const float dy = offset[static_cast<size_t>(2 * tap) * plane + p];
    const float dx = offset[static_cast<size_t>(2 * tap + 1) * plane + p];
    const float mask_logit = modulator[static_cast<size_t>(tap) * plane + p];
    const float mask = 2.0f / (1.0f + expf(-mask_logit));
    const float sy = static_cast<float>(static_cast<int>(oy) - static_cast<int>(padding)
        + static_cast<int>(ky)) + dy;
    const float sx = static_cast<float>(static_cast<int>(ox) - static_cast<int>(padding)
        + static_cast<int>(kx)) + dx;
    const float sampled = makepad_cuda_birefnet_bilinear(
        input + static_cast<size_t>(channel) * plane, height, width, sy, sx);
    output[static_cast<size_t>(col) * m_chunk + row] = __float2half_rn(sampled * mask);
}

extern "C" cudaError_t makepad_cuda_birefnet_deform_im2col_f32_to_f16(
        const float * input,
        const float * offset,
        const float * modulator,
        uint16_t * output,
        uint32_t width,
        uint32_t height,
        uint32_t channels,
        uint32_t kernel,
        uint32_t padding,
        uint32_t p0,
        uint32_t m_chunk,
        cudaStream_t stream) {
    if (!width || !height || !channels || !kernel || !m_chunk) return cudaErrorInvalidValue;
    const uint32_t k_total = channels * kernel * kernel;
    const dim3 block(256, 1, 1);
    const dim3 grid((m_chunk + block.x - 1) / block.x, k_total, 1);
    makepad_cuda_birefnet_deform_im2col_f32_to_f16_kernel<<<grid, block, 0, stream>>>(
        input, offset, modulator, reinterpret_cast<__half *>(output), width, height,
        kernel, padding, p0, m_chunk);
    return cudaGetLastError();
}

// One warp owns one (window, head, query) row.  Swin-L has head_dim=32 and
// window_tokens=144, so materializing the O(nW*heads*144^2) score tensor is
// avoidable and this fused kernel remains comfortably bounded at stage zero.
static __global__ void makepad_cuda_birefnet_swin_attention_f32_kernel(
        const float * __restrict__ q,
        const float * __restrict__ k,
        const float * __restrict__ v,
        const float * __restrict__ relative_bias,
        const uint32_t * __restrict__ regions,
        float * __restrict__ output,
        uint32_t heads,
        uint32_t window_tokens,
        uint32_t head_dim,
        float scale) {
    const uint32_t work = blockIdx.x;
    const uint32_t query_pos = work % window_tokens;
    const uint32_t wh = work / window_tokens;
    const uint32_t head = wh % heads;
    const uint32_t window = wh / heads;
    const uint32_t dim = threadIdx.x;
    const uint32_t hidden = heads * head_dim;
    const uint32_t query_row = window * window_tokens + query_pos;
    const size_t q_index = static_cast<size_t>(query_row) * hidden
        + static_cast<size_t>(head) * head_dim + dim;
    const float q_value = q[q_index];
    __shared__ float scores[144];
    __shared__ float row_max;
    __shared__ float row_sum;
    for (uint32_t key_pos = 0; key_pos < window_tokens; ++key_pos) {
        const uint32_t key_row = window * window_tokens + key_pos;
        const size_t k_index = static_cast<size_t>(key_row) * hidden
            + static_cast<size_t>(head) * head_dim + dim;
        float dot = makepad_cuda_diff_warp_reduce_sum(q_value * k[k_index]);
        if (dim == 0) {
            float score = dot * scale
                + relative_bias[(static_cast<size_t>(head) * window_tokens + query_pos)
                    * window_tokens + key_pos];
            if (regions && regions[query_row] != regions[key_row]) score -= 100.0f;
            scores[key_pos] = score;
        }
    }
    __syncwarp();
    if (dim == 0) {
        float maximum = -CUDART_INF_F;
        for (uint32_t key_pos = 0; key_pos < window_tokens; ++key_pos) {
            maximum = fmaxf(maximum, scores[key_pos]);
        }
        float sum = 0.0f;
        for (uint32_t key_pos = 0; key_pos < window_tokens; ++key_pos) {
            const float p = expf(scores[key_pos] - maximum);
            scores[key_pos] = p;
            sum += p;
        }
        row_max = maximum;
        row_sum = sum;
    }
    __syncwarp();
    (void)row_max;
    float accumulator = 0.0f;
    for (uint32_t key_pos = 0; key_pos < window_tokens; ++key_pos) {
        const uint32_t key_row = window * window_tokens + key_pos;
        const size_t v_index = static_cast<size_t>(key_row) * hidden
            + static_cast<size_t>(head) * head_dim + dim;
        accumulator += (scores[key_pos] / row_sum) * v[v_index];
    }
    output[q_index] = accumulator;
}

extern "C" cudaError_t makepad_cuda_birefnet_swin_attention_f32(
        const float * q,
        const float * k,
        const float * v,
        const float * relative_bias,
        const uint32_t * regions,
        float * output,
        uint32_t windows,
        uint32_t heads,
        uint32_t window_tokens,
        uint32_t head_dim,
        float scale,
        cudaStream_t stream) {
    if (!windows || !heads || !window_tokens || head_dim != 32 || window_tokens > 144) {
        return cudaErrorInvalidValue;
    }
    const uint32_t work = windows * heads * window_tokens;
    makepad_cuda_birefnet_swin_attention_f32_kernel<<<work, head_dim, 0, stream>>>(
        q, k, v, relative_bias, regions, output, heads, window_tokens, head_dim, scale);
    return cudaGetLastError();
}

// ---------------------------------------------------------------------------
// SAM3 DETR decoder helpers: keep the 6-layer reference-point loop fully
// device-resident (sine query embed, axial box-RPB inputs, sigmoid-space box
// refinement). Math mirrors the former host helpers in sam3_model.rs.

static __global__ void makepad_cuda_sam3_sine_embed_f32_kernel(
        const float * __restrict__ ref,
        float * __restrict__ out,
        uint32_t queries,
        uint32_t half) {
    const uint32_t i = blockIdx.x * blockDim.x + threadIdx.x;
    const uint32_t pairs = half / 2;
    const uint32_t total = queries * 4u * pairs;
    if (i >= total) return;
    const uint32_t pair = i % pairs;
    const uint32_t coord = (i / pairs) % 4u;
    const uint32_t q = i / (pairs * 4u);
    const float value = ref[q * 4u + coord];
    const float tau = 6.2831853071795864769f;
    const float freq = powf(10000.0f, (float)(2u * pair) / (float)half);
    const float raw = value * tau / freq;
    float * dest = out + ((size_t)q * 4u + coord) * half;
    dest[2u * pair] = sinf(raw);
    dest[2u * pair + 1u] = cosf(raw);
}

extern "C" cudaError_t makepad_cuda_sam3_sine_embed_f32(
        const float * ref,
        float * out,
        uint32_t queries,
        uint32_t half,
        cudaStream_t stream) {
    if (!queries || !half || (half & 1u)) {
        return cudaErrorInvalidValue;
    }
    const uint32_t total = queries * 4u * (half / 2u);
    const dim3 block(256, 1, 1);
    const dim3 grid((total + block.x - 1) / block.x, 1, 1);
    makepad_cuda_sam3_sine_embed_f32_kernel<<<grid, block, 0, stream>>>(
        ref, out, queries, half);
    return cudaGetLastError();
}

static __device__ __forceinline__ float makepad_cuda_sam3_log_scale(float d) {
    return copysignf(1.0f, d * 8.0f) * log2f(fabsf(d) * 8.0f + 1.0f)
        * 0.33333333333333333f;
}

static __global__ void makepad_cuda_sam3_rpb_axial_f32_kernel(
        const float * __restrict__ ref,
        float * __restrict__ dx,
        float * __restrict__ dy,
        uint32_t queries,
        uint32_t width,
        uint32_t height) {
    const uint32_t i = blockIdx.x * blockDim.x + threadIdx.x;
    const uint32_t per_q = width + height;
    if (i >= queries * per_q) return;
    const uint32_t q = i / per_q;
    const uint32_t r = i - q * per_q;
    const float cx = ref[q * 4u];
    const float cy = ref[q * 4u + 1u];
    const float bw = ref[q * 4u + 2u];
    const float bh = ref[q * 4u + 3u];
    if (r < width) {
        const float xw = (float)r / (float)width;
        float * row = dx + ((size_t)q * width + r) * 2u;
        row[0] = makepad_cuda_sam3_log_scale(xw - (cx - 0.5f * bw));
        row[1] = makepad_cuda_sam3_log_scale(xw - (cx + 0.5f * bw));
    } else {
        const uint32_t y = r - width;
        const float yh = (float)y / (float)height;
        float * row = dy + ((size_t)q * height + y) * 2u;
        row[0] = makepad_cuda_sam3_log_scale(yh - (cy - 0.5f * bh));
        row[1] = makepad_cuda_sam3_log_scale(yh - (cy + 0.5f * bh));
    }
}

extern "C" cudaError_t makepad_cuda_sam3_rpb_axial_f32(
        const float * ref,
        float * dx,
        float * dy,
        uint32_t queries,
        uint32_t width,
        uint32_t height,
        cudaStream_t stream) {
    if (!queries || !width || !height) {
        return cudaErrorInvalidValue;
    }
    const uint32_t total = queries * (width + height);
    const dim3 block(256, 1, 1);
    const dim3 grid((total + block.x - 1) / block.x, 1, 1);
    makepad_cuda_sam3_rpb_axial_f32_kernel<<<grid, block, 0, stream>>>(
        ref, dx, dy, queries, width, height);
    return cudaGetLastError();
}

static __global__ void makepad_cuda_sam3_refine_boxes_f32_kernel(
        const float * __restrict__ ref,
        const float * __restrict__ delta,
        float * __restrict__ out,
        uint32_t n) {
    const uint32_t i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= n) return;
    float x = ref[i];
    x = fminf(fmaxf(x, 1e-6f), 1.0f - 1e-6f);
    const float inv = logf(x / ((1.0f - x) + 1e-6f) + 1e-6f);
    const float z = inv + delta[i];
    out[i] = 1.0f / (1.0f + expf(-z));
}

extern "C" cudaError_t makepad_cuda_sam3_refine_boxes_f32(
        const float * ref,
        const float * delta,
        float * out,
        uint32_t n,
        cudaStream_t stream) {
    if (!n) {
        return cudaErrorInvalidValue;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid((n + block.x - 1) / block.x, 1, 1);
    makepad_cuda_sam3_refine_boxes_f32_kernel<<<grid, block, 0, stream>>>(
        ref, delta, out, n);
    return cudaGetLastError();
}

// ---------------------------------------------------------------------------
// FA2-style register-level flash attention for head_dim 64 (SAM3 ViT trunk).
// Same recipe as the d128 FA2 kernel above: per-warp 16 query rows, Q in
// A-fragments, S/P/O in mma fragments, online softmax on fragment lanes,
// double-buffered cp.async K/V rings; f16 gemm inputs, f32 softmax and
// accumulators, final 1/l normalize.

constexpr int FA64_D = 64;
constexpr int FA64_LDQ = FA64_D + 8;  // 144B row stride: conflict-free x4 ldmatrix
constexpr int FA64_BR = 128;          // query rows per block (8 warps x 16 rows)
constexpr int FA64_BC = 64;           // key rows per tile iteration
constexpr int FA64_THREADS = 256;
constexpr int FA64_STAGE = FA64_BC * FA64_LDQ;
constexpr size_t FA64_SMEM_TOTAL =
    4 * static_cast<size_t>(FA64_STAGE) * sizeof(__half); // K ring x2 + V ring x2

static __device__ __forceinline__ void makepad_cuda_fa64_tile_async(
        const __half * __restrict__ src,
        __half * __restrict__ dst,
        uint32_t row0,
        uint32_t seq,
        uint32_t hidden,
        uint32_t col0) {
    const int r = threadIdx.x >> 2;      // 64 rows, 4 threads per row
    const int quarter = threadIdx.x & 3; // 16 halves per thread
    const uint32_t row = row0 + r;
    const int src_bytes = row < seq ? 16 : 0;
    const size_t src_row = row < seq ? row : 0;
    const __half * in = src + src_row * hidden + col0 + quarter * 16;
    __half * out = dst + r * FA64_LDQ + quarter * 16;
    #pragma unroll
    for (int i = 0; i < 2; i++) {
        makepad_cuda_fa_cp_async16(out + i * 8, in + i * 8, src_bytes);
    }
}

static __global__ void makepad_cuda_flash_attention2_d64_f32_kernel(
        const __half * __restrict__ q,
        const __half * __restrict__ k,
        const __half * __restrict__ v,
        float * __restrict__ out,
        uint32_t seq,
        uint32_t kv_len,
        uint32_t hidden,
        float scale) {
    extern __shared__ __align__(16) char fa64_smem[];
    __half * k_ring = reinterpret_cast<__half *>(fa64_smem);
    __half * v_ring = k_ring + 2 * FA64_STAGE;

    const uint32_t q0 = blockIdx.x * FA64_BR;
    const uint32_t col0 = blockIdx.y * FA64_D;
    if (q0 >= seq) {
        return;
    }

    const int warp = threadIdx.x >> 5;
    const int lane = threadIdx.x & 31;
    const int lane_row = lane >> 2;
    const int lane_quad = lane & 3;

    // Prologue: stage Q through the K ring, then park it in registers.
    makepad_cuda_fa64_tile_async(q, k_ring, q0, seq, hidden, col0);
    makepad_cuda_fa64_tile_async(q, k_ring + FA64_STAGE, q0 + FA64_BC, seq, hidden, col0);
    makepad_cuda_fa_cp_commit();
    makepad_cuda_fa_cp_wait<0>();
    __syncthreads();

    uint32_t q_frag[4][4]; // 16 x 64 as 4 k-chunks of m16k16
    {
        const int row_in_stage = (warp * 16) & (FA64_BC - 1);
        const __half * q_sh = k_ring + (warp >= 4 ? FA64_STAGE : 0)
            + row_in_stage * FA64_LDQ;
        #pragma unroll
        for (int kk = 0; kk < 4; kk++) {
            const __half * addr = q_sh + (lane & 15) * FA64_LDQ + kk * 16 + (lane >> 4) * 8;
            makepad_cuda_fa2_ldmatrix_x4(q_frag[kk], addr);
        }
    }
    __syncthreads();

    float o_acc[8][4]; // 16 x 64 output as 8 n8 accumulator fragments
    #pragma unroll
    for (int j = 0; j < 8; j++) {
        o_acc[j][0] = 0.0f;
        o_acc[j][1] = 0.0f;
        o_acc[j][2] = 0.0f;
        o_acc[j][3] = 0.0f;
    }
    float m_lo = -CUDART_INF_F, m_hi = -CUDART_INF_F;
    float l_lo = 0.0f, l_hi = 0.0f;

    const uint32_t tiles = (kv_len + FA64_BC - 1) / FA64_BC;
    makepad_cuda_fa64_tile_async(k, k_ring, 0, kv_len, hidden, col0);
    makepad_cuda_fa64_tile_async(v, v_ring, 0, kv_len, hidden, col0);
    makepad_cuda_fa_cp_commit();

    for (uint32_t tile = 0; tile < tiles; tile++) {
        const uint32_t k0 = tile * FA64_BC;
        const int stage = tile & 1;
        if (tile + 1 < tiles) {
            makepad_cuda_fa64_tile_async(
                k, k_ring + (stage ^ 1) * FA64_STAGE, k0 + FA64_BC, kv_len, hidden, col0);
            makepad_cuda_fa64_tile_async(
                v, v_ring + (stage ^ 1) * FA64_STAGE, k0 + FA64_BC, kv_len, hidden, col0);
            makepad_cuda_fa_cp_commit();
            makepad_cuda_fa_cp_wait<1>();
        } else {
            makepad_cuda_fa_cp_wait<0>();
        }
        __syncthreads();

        const __half * k_sh = k_ring + stage * FA64_STAGE;
        const __half * v_sh = v_ring + stage * FA64_STAGE;

        float s[8][4];
        #pragma unroll
        for (int j = 0; j < 8; j++) {
            s[j][0] = 0.0f;
            s[j][1] = 0.0f;
            s[j][2] = 0.0f;
            s[j][3] = 0.0f;
        }
        #pragma unroll
        for (int j = 0; j < 8; j += 2) {
            #pragma unroll
            for (int kk = 0; kk < 4; kk++) {
                const int sel = lane >> 3;
                const __half * addr = k_sh
                    + (8 * (j + (sel >> 1)) + (lane & 7)) * FA64_LDQ
                    + kk * 16 + (sel & 1) * 8;
                uint32_t b[4];
                makepad_cuda_fa2_ldmatrix_x4(b, addr);
                makepad_cuda_fa2_mma(s[j], q_frag[kk], b[0], b[1]);
                makepad_cuda_fa2_mma(s[j + 1], q_frag[kk], b[2], b[3]);
            }
        }

        const uint32_t remaining = kv_len - k0;
        const uint32_t valid = remaining < FA64_BC ? remaining : FA64_BC;
        float tmax_lo = -CUDART_INF_F, tmax_hi = -CUDART_INF_F;
        #pragma unroll
        for (int j = 0; j < 8; j++) {
            const uint32_t c = 8 * j + 2 * lane_quad;
            s[j][0] = c < valid ? s[j][0] * scale : -CUDART_INF_F;
            s[j][1] = c + 1 < valid ? s[j][1] * scale : -CUDART_INF_F;
            s[j][2] = c < valid ? s[j][2] * scale : -CUDART_INF_F;
            s[j][3] = c + 1 < valid ? s[j][3] * scale : -CUDART_INF_F;
            tmax_lo = fmaxf(tmax_lo, fmaxf(s[j][0], s[j][1]));
            tmax_hi = fmaxf(tmax_hi, fmaxf(s[j][2], s[j][3]));
        }
        tmax_lo = fmaxf(tmax_lo, __shfl_xor_sync(0xffffffffu, tmax_lo, 1));
        tmax_lo = fmaxf(tmax_lo, __shfl_xor_sync(0xffffffffu, tmax_lo, 2));
        tmax_hi = fmaxf(tmax_hi, __shfl_xor_sync(0xffffffffu, tmax_hi, 1));
        tmax_hi = fmaxf(tmax_hi, __shfl_xor_sync(0xffffffffu, tmax_hi, 2));
        const float m_new_lo = m_lo > tmax_lo ? m_lo : tmax_lo;
        const float m_new_hi = m_hi > tmax_hi ? m_hi : tmax_hi;
        const float rescale_lo = expf(m_lo - m_new_lo);
        const float rescale_hi = expf(m_hi - m_new_hi);
        float sum_lo = 0.0f, sum_hi = 0.0f;
        uint32_t p_frag[4][4];
        #pragma unroll
        for (int kk2 = 0; kk2 < 4; kk2++) {
            const int j0 = 2 * kk2;
            const float p00 = expf(s[j0][0] - m_new_lo);
            const float p01 = expf(s[j0][1] - m_new_lo);
            const float p02 = expf(s[j0][2] - m_new_hi);
            const float p03 = expf(s[j0][3] - m_new_hi);
            const float p10 = expf(s[j0 + 1][0] - m_new_lo);
            const float p11 = expf(s[j0 + 1][1] - m_new_lo);
            const float p12 = expf(s[j0 + 1][2] - m_new_hi);
            const float p13 = expf(s[j0 + 1][3] - m_new_hi);
            sum_lo += p00 + p01 + p10 + p11;
            sum_hi += p02 + p03 + p12 + p13;
            p_frag[kk2][0] = makepad_cuda_fa2_pack(p00, p01);
            p_frag[kk2][1] = makepad_cuda_fa2_pack(p02, p03);
            p_frag[kk2][2] = makepad_cuda_fa2_pack(p10, p11);
            p_frag[kk2][3] = makepad_cuda_fa2_pack(p12, p13);
        }
        sum_lo += __shfl_xor_sync(0xffffffffu, sum_lo, 1);
        sum_lo += __shfl_xor_sync(0xffffffffu, sum_lo, 2);
        sum_hi += __shfl_xor_sync(0xffffffffu, sum_hi, 1);
        sum_hi += __shfl_xor_sync(0xffffffffu, sum_hi, 2);
        l_lo = l_lo * rescale_lo + sum_lo;
        l_hi = l_hi * rescale_hi + sum_hi;
        m_lo = m_new_lo;
        m_hi = m_new_hi;
        #pragma unroll
        for (int j = 0; j < 8; j++) {
            o_acc[j][0] *= rescale_lo;
            o_acc[j][1] *= rescale_lo;
            o_acc[j][2] *= rescale_hi;
            o_acc[j][3] *= rescale_hi;
        }

        #pragma unroll
        for (int jj = 0; jj < 8; jj += 2) {
            #pragma unroll
            for (int kk2 = 0; kk2 < 4; kk2++) {
                const int sel = lane >> 3;
                const __half * addr = v_sh
                    + (16 * kk2 + 8 * (sel & 1) + (lane & 7)) * FA64_LDQ
                    + 8 * (jj + (sel >> 1));
                uint32_t b[4];
                makepad_cuda_fa2_ldmatrix_x4_trans(b, addr);
                makepad_cuda_fa2_mma(o_acc[jj], p_frag[kk2], b[0], b[1]);
                makepad_cuda_fa2_mma(o_acc[jj + 1], p_frag[kk2], b[2], b[3]);
            }
        }
        __syncthreads();
    }

    const uint32_t row_lo = q0 + warp * 16 + lane_row;
    const uint32_t row_hi = row_lo + 8;
    const float inv_lo = l_lo > 0.0f ? 1.0f / l_lo : 0.0f;
    const float inv_hi = l_hi > 0.0f ? 1.0f / l_hi : 0.0f;
    if (row_lo < seq) {
        float * dst = out + static_cast<size_t>(row_lo) * hidden + col0 + 2 * lane_quad;
        #pragma unroll
        for (int j = 0; j < 8; j++) {
            float2 f;
            f.x = o_acc[j][0] * inv_lo;
            f.y = o_acc[j][1] * inv_lo;
            *reinterpret_cast<float2 *>(dst + 8 * j) = f;
        }
    }
    if (row_hi < seq) {
        float * dst = out + static_cast<size_t>(row_hi) * hidden + col0 + 2 * lane_quad;
        #pragma unroll
        for (int j = 0; j < 8; j++) {
            float2 f;
            f.x = o_acc[j][2] * inv_hi;
            f.y = o_acc[j][3] * inv_hi;
            *reinterpret_cast<float2 *>(dst + 8 * j) = f;
        }
    }
}

extern "C" cudaError_t makepad_cuda_flash_attention2_d64_f32(
        const uint16_t * q,
        const uint16_t * k,
        const uint16_t * v,
        float * out,
        uint32_t seq,
        uint32_t kv_len,
        uint32_t head_count,
        uint32_t hidden,
        float scale,
        cudaStream_t stream) {
    if (seq == 0 || head_count == 0) {
        return cudaSuccess;
    }
    if (kv_len == 0) {
        return cudaErrorInvalidValue;
    }
    if (hidden != head_count * FA64_D) {
        return cudaErrorInvalidValue;
    }
    const dim3 block(FA64_THREADS, 1, 1);
    const dim3 grid((seq + FA64_BR - 1) / FA64_BR, head_count, 1);
    makepad_cuda_flash_attention2_d64_f32_kernel
        <<<grid, block, FA64_SMEM_TOTAL, stream>>>(
            reinterpret_cast<const __half *>(q),
            reinterpret_cast<const __half *>(k),
            reinterpret_cast<const __half *>(v),
            out, seq, kv_len, hidden, scale);
    return cudaGetLastError();
}

// RealESRGAN x4plus primitives.  Feature maps are planar [channel, y*x]
// (batch 1), the same decoder layout as the BiRefNet block above.  Both ops
// are pure elementwise passes; convolutions ride the shared conv2d paths.

static __global__ void makepad_cuda_realesrgan_lrelu_f32_kernel(
        const float * __restrict__ input,
        float * __restrict__ output,
        size_t n,
        float slope) {
    const size_t i = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    if (i < n) {
        const float value = input[i];
        output[i] = value > 0.0f ? value : value * slope;
    }
}

extern "C" cudaError_t makepad_cuda_realesrgan_lrelu_f32(
        const float * input,
        float * output,
        size_t n,
        float slope,
        cudaStream_t stream) {
    if (n == 0) return cudaSuccess;
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<unsigned int>((n + block.x - 1) / block.x), 1, 1);
    makepad_cuda_realesrgan_lrelu_f32_kernel<<<grid, block, 0, stream>>>(
        input, output, n, slope);
    return cudaGetLastError();
}

static __global__ void makepad_cuda_realesrgan_scale_add_f32_kernel(
        const float * __restrict__ base,
        const float * __restrict__ delta,
        float * __restrict__ output,
        size_t n,
        float scale) {
    const size_t i = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    if (i < n) {
        output[i] = fmaf(scale, delta[i], base[i]);
    }
}

extern "C" cudaError_t makepad_cuda_realesrgan_scale_add_f32(
        const float * base,
        const float * delta,
        float * output,
        size_t n,
        float scale,
        cudaStream_t stream) {
    if (n == 0) return cudaSuccess;
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<unsigned int>((n + block.x - 1) / block.x), 1, 1);
    makepad_cuda_realesrgan_scale_add_f32_kernel<<<grid, block, 0, stream>>>(
        base, delta, output, n, scale);
    return cudaGetLastError();
}

// RealESRGAN f16 fast path.  The dense blocks live in one persistent wide
// planar f16 buffer; cuDNN convs write raw (biasless) rows into it and these
// epilogues fold bias, LeakyReLU, and the 0.2-scaled residuals in place.

static __global__ void makepad_cuda_realesrgan_bias_lrelu_f16_kernel(
        __half * __restrict__ data,
        const float * __restrict__ bias,
        size_t plane,
        size_t n,
        float slope) {
    const size_t i = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    if (i < n) {
        const float value = __half2float(data[i]) + bias[i / plane];
        data[i] = __float2half(value > 0.0f ? value : value * slope);
    }
}

extern "C" cudaError_t makepad_cuda_realesrgan_bias_lrelu_f16(
        __half * data,
        const float * bias,
        size_t plane,
        size_t n,
        float slope,
        cudaStream_t stream) {
    if (n == 0) return cudaSuccess;
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<unsigned int>((n + block.x - 1) / block.x), 1, 1);
    makepad_cuda_realesrgan_bias_lrelu_f16_kernel<<<grid, block, 0, stream>>>(
        data, bias, plane, n, slope);
    return cudaGetLastError();
}

// f32 spine residual: dst32 = base + scale * (delta + bias) with the delta
// read from either an f32 or an f16 operand, optionally mirroring the result
// into the f16 working buffer (the conv-input view).  Keeping the RRDB/trunk
// accumulation chain in f32 stops spine rounding from compounding across the
// 23 blocks.
static __global__ void makepad_cuda_realesrgan_spine_axpb_kernel(
        const float * __restrict__ base,
        const float * __restrict__ delta32,
        const __half * __restrict__ delta16,
        const float * __restrict__ bias,
        float * __restrict__ dst32,
        __half * __restrict__ dst16,
        size_t plane,
        size_t n,
        float scale) {
    const size_t i = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    if (i < n) {
        float d = delta32 != nullptr ? delta32[i] : __half2float(delta16[i]);
        if (bias != nullptr) {
            d += bias[i / plane];
        }
        const float value = fmaf(scale, d, base[i]);
        dst32[i] = value;
        if (dst16 != nullptr) {
            dst16[i] = __float2half(value);
        }
    }
}

extern "C" cudaError_t makepad_cuda_realesrgan_spine_axpb(
        const float * base,
        const float * delta32,
        const __half * delta16,
        const float * bias,
        float * dst32,
        __half * dst16,
        size_t plane,
        size_t n,
        float scale,
        cudaStream_t stream) {
    if (n == 0) return cudaSuccess;
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<unsigned int>((n + block.x - 1) / block.x), 1, 1);
    makepad_cuda_realesrgan_spine_axpb_kernel<<<grid, block, 0, stream>>>(
        base, delta32, delta16, bias, dst32, dst16, plane, n, scale);
    return cudaGetLastError();
}

// f32 twin of the bias+LeakyReLU epilogue for the true-f32 head convs.
static __global__ void makepad_cuda_realesrgan_bias_lrelu_f32_kernel(
        float * __restrict__ data,
        const float * __restrict__ bias,
        size_t plane,
        size_t n,
        float slope) {
    const size_t i = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    if (i < n) {
        const float value = data[i] + bias[i / plane];
        data[i] = value > 0.0f ? value : value * slope;
    }
}

extern "C" cudaError_t makepad_cuda_realesrgan_bias_lrelu_f32(
        float * data,
        const float * bias,
        size_t plane,
        size_t n,
        float slope,
        cudaStream_t stream) {
    if (n == 0) return cudaSuccess;
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<unsigned int>((n + block.x - 1) / block.x), 1, 1);
    makepad_cuda_realesrgan_bias_lrelu_f32_kernel<<<grid, block, 0, stream>>>(
        data, bias, plane, n, slope);
    return cudaGetLastError();
}

static __global__ void makepad_cuda_realesrgan_quantize_rgb8_f32_kernel(
        const float * __restrict__ input,
        unsigned char * __restrict__ output,
        size_t plane) {
    const size_t i = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    if (i < plane) {
        for (int channel = 0; channel < 3; ++channel) {
            const float value =
                fminf(fmaxf(input[channel * plane + i], 0.0f), 1.0f);
            output[i * 3 + channel] =
                static_cast<unsigned char>(roundf(value * 255.0f));
        }
    }
}

extern "C" cudaError_t makepad_cuda_realesrgan_quantize_rgb8_f32(
        const float * input,
        unsigned char * output,
        size_t plane,
        cudaStream_t stream) {
    if (plane == 0) return cudaSuccess;
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<unsigned int>((plane + block.x - 1) / block.x), 1, 1);
    makepad_cuda_realesrgan_quantize_rgb8_f32_kernel<<<grid, block, 0, stream>>>(
        input, output, plane);
    return cudaGetLastError();
}
