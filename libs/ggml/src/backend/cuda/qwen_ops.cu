#include <cuda_runtime.h>
#include <math.h>
#include <stdint.h>

static __device__ __forceinline__ float qwen_sigmoid_f32(const float value) {
    return 1.0f / (1.0f + __expf(-value));
}

static __device__ __forceinline__ float qwen_silu_f32(const float value) {
    return value / (1.0f + __expf(-value));
}

static __device__ __forceinline__ float qwen_softplus_f32(const float value) {
    if (value > 20.0f) {
        return value;
    }
    if (value < -20.0f) {
        return __expf(value);
    }
    return log1pf(__expf(value));
}

static __global__ void makepad_ggml_cuda_qwen_split_interleaved_query_gate_f32_kernel(
    const float * __restrict__ input,
    float * __restrict__ query,
    float * __restrict__ gate,
    uint32_t head_count,
    uint32_t head_dim
) {
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    const uint32_t width = head_count * head_dim;
    if (idx >= width) {
        return;
    }
    const uint32_t head = idx / head_dim;
    const uint32_t dim = idx % head_dim;
    const uint32_t src = head * head_dim * 2 + dim;
    query[idx] = input[src];
    gate[idx] = input[src + head_dim];
}

static __global__ void makepad_ggml_cuda_qwen_split_recurrent_qkv_f32_kernel(
    const float * __restrict__ input,
    float * __restrict__ q,
    float * __restrict__ k,
    float * __restrict__ v,
    uint32_t q_width,
    uint32_t v_width
) {
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    const uint32_t total = q_width * 2 + v_width;
    if (idx >= total) {
        return;
    }
    if (idx < q_width) {
        q[idx] = input[idx];
    } else if (idx < q_width * 2) {
        k[idx - q_width] = input[idx];
    } else {
        v[idx - q_width * 2] = input[idx];
    }
}

static __global__ void makepad_ggml_cuda_qwen_sigmoid_f32_kernel(
    const float * __restrict__ input,
    float * __restrict__ output,
    uint32_t n
) {
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx >= n) {
        return;
    }
    output[idx] = qwen_sigmoid_f32(input[idx]);
}

static __global__ void makepad_ggml_cuda_qwen_sigmoid_mul_f32_kernel(
    const float * __restrict__ values,
    const float * __restrict__ gate,
    float * __restrict__ output,
    uint32_t n
) {
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx >= n) {
        return;
    }
    output[idx] = values[idx] * qwen_sigmoid_f32(gate[idx]);
}

static __global__ void makepad_ggml_cuda_qwen_silu_mul_f32_kernel(
    const float * __restrict__ values,
    const float * __restrict__ gate,
    float * __restrict__ output,
    uint32_t n
) {
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx >= n) {
        return;
    }
    output[idx] = values[idx] * qwen_silu_f32(gate[idx]);
}

static __global__ void makepad_ggml_cuda_qwen_swiglu_split_f32_kernel(
    const float * __restrict__ gate_up,
    float * __restrict__ output,
    uint32_t n,
    uint32_t split_offset
) {
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx >= n) {
        return;
    }
    output[idx] = qwen_silu_f32(gate_up[idx]) * gate_up[idx + split_offset];
}

static __global__ void makepad_ggml_cuda_qwen_decay_gate_f32_kernel(
    const float * __restrict__ a_log,
    const float * __restrict__ alpha,
    const float * __restrict__ dt_bias,
    float * __restrict__ output,
    uint32_t n
) {
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx >= n) {
        return;
    }
    output[idx] = -__expf(a_log[idx]) * qwen_softplus_f32(alpha[idx] + dt_bias[idx]);
}

static __global__ void makepad_ggml_cuda_qwen_ssm_conv_with_state_f32_kernel(
    const float * __restrict__ current,
    float * __restrict__ state,
    const float * __restrict__ kernel,
    float * __restrict__ output,
    uint32_t d_conv,
    uint32_t d_inner
) {
    const uint32_t channel = blockIdx.x * blockDim.x + threadIdx.x;
    if (channel >= d_inner) {
        return;
    }
    const uint32_t prefix = d_conv > 0 ? d_conv - 1 : 0;
    const uint32_t state_base = channel * prefix;
    const uint32_t kernel_base = channel * d_conv;

    float sum = 0.0f;
    for (uint32_t tap = 0; tap < prefix; ++tap) {
        sum += state[state_base + tap] * kernel[kernel_base + tap];
    }
    sum += current[channel] * kernel[kernel_base + prefix];
    output[channel] = qwen_silu_f32(sum);

    if (prefix != 0) {
        for (uint32_t tap = 0; tap + 1 < prefix; ++tap) {
            state[state_base + tap] = state[state_base + tap + 1];
        }
        state[state_base + prefix - 1] = current[channel];
    }
}

static __global__ void makepad_ggml_cuda_qwen_mrope_rows_f32_kernel(
    const float * __restrict__ input,
    float * __restrict__ output,
    uint32_t row_count,
    uint32_t row_stride,
    uint32_t rotary_dim,
    float rope_theta,
    uint32_t position0,
    uint32_t position1,
    uint32_t position2,
    uint32_t position3,
    uint32_t section0,
    uint32_t section1,
    uint32_t section2,
    uint32_t section3
) {
    const uint32_t pair_count = rotary_dim / 2;
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    const uint32_t total = row_count * pair_count;
    if (idx >= total) {
        return;
    }

    const uint32_t row = idx / pair_count;
    const uint32_t pair_idx = idx % pair_count;
    const uint32_t base = row * row_stride;

    const uint32_t sect_dims = section0 + section1 + section2 + section3;
    if (sect_dims == 0) {
        return;
    }
    const uint32_t sector = pair_idx % sect_dims;
    const uint32_t section_h_start = section0;
    const uint32_t section_w_start = section_h_start + section1;
    const uint32_t section_e_start = section_w_start + section2;

    uint32_t position = position0;
    if (sector % 3 == 1 && sector < 3 * section1) {
        position = position1;
    } else if (sector % 3 == 2 && sector < 3 * section2) {
        position = position2;
    } else if (sector % 3 == 0 && sector < 3 * section0) {
        position = position0;
    } else if (sector >= section_e_start) {
        position = position3;
    } else if (sector >= section_w_start) {
        position = position2;
    } else if (sector >= section_h_start) {
        position = position1;
    }

    const float theta = position * powf(rope_theta, -(2.0f * pair_idx) / (float) rotary_dim);
    const float cos_theta = cosf(theta);
    const float sin_theta = sinf(theta);
    const float x0 = input[base + pair_idx];
    const float x1 = input[base + pair_idx + pair_count];
    output[base + pair_idx] = x0 * cos_theta - x1 * sin_theta;
    output[base + pair_idx + pair_count] = x0 * sin_theta + x1 * cos_theta;
}

static __global__ void makepad_ggml_cuda_qwen_mrope_rows_f32_device_u32_kernel(
    const float * __restrict__ input,
    float * __restrict__ output,
    uint32_t row_count,
    uint32_t row_stride,
    uint32_t rotary_dim,
    float rope_theta,
    const uint32_t * __restrict__ position_device_u32,
    uint32_t section0,
    uint32_t section1,
    uint32_t section2,
    uint32_t section3
) {
    const uint32_t position = *position_device_u32;
    const uint32_t pair_count = rotary_dim / 2;
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    const uint32_t total = row_count * pair_count;
    if (idx >= total) {
        return;
    }

    const uint32_t row = idx / pair_count;
    const uint32_t pair_idx = idx % pair_count;
    const uint32_t base = row * row_stride;

    const uint32_t sect_dims = section0 + section1 + section2 + section3;
    if (sect_dims == 0) {
        return;
    }
    const uint32_t sector = pair_idx % sect_dims;
    const uint32_t section_h_start = section0;
    const uint32_t section_w_start = section_h_start + section1;
    const uint32_t section_e_start = section_w_start + section2;

    uint32_t axis_position = position;
    if (sector >= section_e_start) {
        axis_position = 0;
    } else if (sector >= section_w_start) {
        axis_position = position;
    } else if (sector >= section_h_start) {
        axis_position = position;
    }

    const float theta = axis_position * powf(rope_theta, -(2.0f * pair_idx) / (float) rotary_dim);
    const float cos_theta = cosf(theta);
    const float sin_theta = sinf(theta);
    const float x0 = input[base + pair_idx];
    const float x1 = input[base + pair_idx + pair_count];
    output[base + pair_idx] = x0 * cos_theta - x1 * sin_theta;
    output[base + pair_idx + pair_count] = x0 * sin_theta + x1 * cos_theta;
}

static __global__ void makepad_ggml_cuda_qwen_softmax_topk_routes_f32_kernel(
    const float * __restrict__ logits,
    uint32_t * __restrict__ topk_indices,
    float * __restrict__ topk_weights,
    uint32_t n,
    uint32_t top_k
) {
    __shared__ float shared_logits[256];
    __shared__ float selected_logits[256];
    if (threadIdx.x != 0) {
        return;
    }

    for (uint32_t expert = 0; expert < n; ++expert) {
        shared_logits[expert] = logits[expert];
    }

    for (uint32_t slot = 0; slot < top_k; ++slot) {
        float best_logit = -INFINITY;
        uint32_t best_index = 0;
        for (uint32_t expert = 0; expert < n; ++expert) {
            const float candidate_logit = shared_logits[expert];
            if (candidate_logit > best_logit ||
                    (candidate_logit == best_logit && expert < best_index)) {
                best_logit = candidate_logit;
                best_index = expert;
            }
        }
        topk_indices[slot] = best_index;
        selected_logits[slot] = best_logit;
        shared_logits[best_index] = -INFINITY;
    }

    float selected_max_logit = selected_logits[0];
    for (uint32_t slot = 1; slot < top_k; ++slot) {
        selected_max_logit = fmaxf(selected_max_logit, selected_logits[slot]);
    }

    double selected_sum = 0.0;
    for (uint32_t slot = 0; slot < top_k; ++slot) {
        const float selected_prob = static_cast<float>(
            exp(static_cast<double>(selected_logits[slot] - selected_max_logit)));
        topk_weights[slot] = selected_prob;
        selected_sum += static_cast<double>(selected_prob);
    }

    const float inv_selected_sum =
        selected_sum > 0.0 && isfinite(selected_sum) ? static_cast<float>(1.0 / selected_sum) : 0.0f;
    for (uint32_t slot = 0; slot < top_k; ++slot) {
        topk_weights[slot] *= inv_selected_sum;
    }
}

extern "C" cudaError_t makepad_ggml_cuda_qwen_split_interleaved_query_gate_f32(
    const float * input,
    float * query,
    float * gate,
    uint32_t head_count,
    uint32_t head_dim,
    cudaStream_t stream
) {
    const uint32_t total = head_count * head_dim;
    const dim3 block(256, 1, 1);
    const dim3 grid((total + block.x - 1) / block.x, 1, 1);
    makepad_ggml_cuda_qwen_split_interleaved_query_gate_f32_kernel<<<grid, block, 0, stream>>>(
        input, query, gate, head_count, head_dim
    );
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_qwen_split_recurrent_qkv_f32(
    const float * input,
    float * q,
    float * k,
    float * v,
    uint32_t q_width,
    uint32_t v_width,
    cudaStream_t stream
) {
    const uint32_t total = q_width * 2 + v_width;
    const dim3 block(256, 1, 1);
    const dim3 grid((total + block.x - 1) / block.x, 1, 1);
    makepad_ggml_cuda_qwen_split_recurrent_qkv_f32_kernel<<<grid, block, 0, stream>>>(
        input, q, k, v, q_width, v_width
    );
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_qwen_sigmoid_f32(
    const float * input,
    float * output,
    uint32_t n,
    cudaStream_t stream
) {
    const dim3 block(256, 1, 1);
    const dim3 grid((n + block.x - 1) / block.x, 1, 1);
    makepad_ggml_cuda_qwen_sigmoid_f32_kernel<<<grid, block, 0, stream>>>(input, output, n);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_qwen_sigmoid_mul_f32(
    const float * values,
    const float * gate,
    float * output,
    uint32_t n,
    cudaStream_t stream
) {
    const dim3 block(256, 1, 1);
    const dim3 grid((n + block.x - 1) / block.x, 1, 1);
    makepad_ggml_cuda_qwen_sigmoid_mul_f32_kernel<<<grid, block, 0, stream>>>(
        values, gate, output, n
    );
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_qwen_silu_mul_f32(
    const float * values,
    const float * gate,
    float * output,
    uint32_t n,
    cudaStream_t stream
) {
    const dim3 block(256, 1, 1);
    const dim3 grid((n + block.x - 1) / block.x, 1, 1);
    makepad_ggml_cuda_qwen_silu_mul_f32_kernel<<<grid, block, 0, stream>>>(
        values, gate, output, n
    );
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_qwen_swiglu_split_f32(
    const float * gate_up,
    float * output,
    uint32_t n,
    uint32_t split_offset,
    cudaStream_t stream
) {
    const dim3 block(256, 1, 1);
    const dim3 grid((n + block.x - 1) / block.x, 1, 1);
    makepad_ggml_cuda_qwen_swiglu_split_f32_kernel<<<grid, block, 0, stream>>>(
        gate_up, output, n, split_offset
    );
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_qwen_decay_gate_f32(
    const float * a_log,
    const float * alpha,
    const float * dt_bias,
    float * output,
    uint32_t n,
    cudaStream_t stream
) {
    const dim3 block(256, 1, 1);
    const dim3 grid((n + block.x - 1) / block.x, 1, 1);
    makepad_ggml_cuda_qwen_decay_gate_f32_kernel<<<grid, block, 0, stream>>>(
        a_log, alpha, dt_bias, output, n
    );
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_qwen_ssm_conv_with_state_f32(
    const float * current,
    float * state,
    const float * kernel,
    float * output,
    uint32_t d_conv,
    uint32_t d_inner,
    cudaStream_t stream
) {
    if (d_conv == 0 || d_inner == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid((d_inner + block.x - 1) / block.x, 1, 1);
    makepad_ggml_cuda_qwen_ssm_conv_with_state_f32_kernel<<<grid, block, 0, stream>>>(
        current, state, kernel, output, d_conv, d_inner
    );
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_qwen_mrope_rows_f32(
    const float * input,
    float * output,
    uint32_t row_count,
    uint32_t row_stride,
    uint32_t rotary_dim,
    float rope_theta,
    uint32_t position0,
    uint32_t position1,
    uint32_t position2,
    uint32_t position3,
    uint32_t section0,
    uint32_t section1,
    uint32_t section2,
    uint32_t section3,
    cudaStream_t stream
) {
    const uint32_t pair_count = rotary_dim / 2;
    const uint32_t total = row_count * pair_count;
    const dim3 block(256, 1, 1);
    const dim3 grid((total + block.x - 1) / block.x, 1, 1);
    makepad_ggml_cuda_qwen_mrope_rows_f32_kernel<<<grid, block, 0, stream>>>(
        input,
        output,
        row_count,
        row_stride,
        rotary_dim,
        rope_theta,
        position0,
        position1,
        position2,
        position3,
        section0,
        section1,
        section2,
        section3
    );
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_qwen_mrope_rows_f32_device_u32(
    const float * input,
    float * output,
    uint32_t row_count,
    uint32_t row_stride,
    uint32_t rotary_dim,
    float rope_theta,
    const uint32_t * position_device_u32,
    uint32_t section0,
    uint32_t section1,
    uint32_t section2,
    uint32_t section3,
    cudaStream_t stream
) {
    const uint32_t pair_count = rotary_dim / 2;
    const uint32_t total = row_count * pair_count;
    const dim3 block(256, 1, 1);
    const dim3 grid((total + block.x - 1) / block.x, 1, 1);
    makepad_ggml_cuda_qwen_mrope_rows_f32_device_u32_kernel<<<grid, block, 0, stream>>>(
        input,
        output,
        row_count,
        row_stride,
        rotary_dim,
        rope_theta,
        position_device_u32,
        section0,
        section1,
        section2,
        section3
    );
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_qwen_softmax_topk_routes_f32(
    const float * logits,
    uint32_t * topk_indices,
    float * topk_weights,
    uint32_t n,
    uint32_t top_k,
    cudaStream_t stream
) {
    if (n == 0 || top_k == 0 || top_k > n || n > 256) {
        return cudaErrorInvalidValue;
    }
    makepad_ggml_cuda_qwen_softmax_topk_routes_f32_kernel<<<1, 256, 0, stream>>>(
        logits,
        topk_indices,
        topk_weights,
        n,
        top_k
    );
    return cudaGetLastError();
}
