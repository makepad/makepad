#include <cuda_runtime.h>
#include <math_constants.h>
#include <math.h>

// All row kernels use 256 threads (eight warps). The result is valid in
// thread zero; callers publish it with a shared scalar and a block barrier.
template<bool Maximum>
static __device__ __forceinline__ float mkwe_reduce(float value) {
    __shared__ float partial[8];
    const int lane = threadIdx.x & 31;
    const int warp = threadIdx.x >> 5;
    #pragma unroll
    for (int offset = 16; offset > 0; offset >>= 1) {
        const float other = __shfl_down_sync(0xffffffffu, value, offset);
        value = Maximum ? fmaxf(value, other) : __fadd_rn(value, other);
    }
    if (lane == 0) {
        partial[warp] = value;
    }
    __syncthreads();
    value = threadIdx.x < 8 ? partial[lane] : (Maximum ? -CUDART_INF_F : 0.0f);
    if (warp == 0) {
        #pragma unroll
        for (int offset = 16; offset > 0; offset >>= 1) {
            const float other = __shfl_down_sync(0xffffffffu, value, offset);
            value = Maximum ? fmaxf(value, other) : __fadd_rn(value, other);
        }
    }
    return value;
}

static __global__ void mkwe_layer_norm_kernel(
        const float * x, const float * weight, const float * bias,
        float * y, int cols) {
    const size_t base = static_cast<size_t>(blockIdx.x) * cols;
    __shared__ float mean;
    __shared__ float inv_std;
    float sum = 0.0f;
    for (unsigned int c = threadIdx.x; c < static_cast<unsigned int>(cols); c += blockDim.x) {
        sum = __fadd_rn(sum, x[base + c]);
    }
    sum = mkwe_reduce<false>(sum);
    if (threadIdx.x == 0) {
        mean = sum / static_cast<float>(cols);
    }
    __syncthreads();

    // CPU semantics: E[(x - mean)^2], not E[x^2] - mean^2.
    float variance = 0.0f;
    for (unsigned int c = threadIdx.x; c < static_cast<unsigned int>(cols); c += blockDim.x) {
        const float delta = x[base + c] - mean;
        variance = __fadd_rn(variance, __fmul_rn(delta, delta));
    }
    variance = mkwe_reduce<false>(variance);
    if (threadIdx.x == 0) {
        inv_std = 1.0f / sqrtf(variance / static_cast<float>(cols) + 1.0e-5f);
    }
    __syncthreads();
    for (unsigned int c = threadIdx.x; c < static_cast<unsigned int>(cols); c += blockDim.x) {
        const float normalized = __fmul_rn(x[base + c] - mean, inv_std);
        // Keep normalization, multiply, and add separate like cpu/tensor.rs.
        y[base + c] = __fadd_rn(__fmul_rn(normalized, weight[c]), bias[c]);
    }
}

template<bool Gelu>
static __global__ void mkwe_bias_kernel(float * y, const float * bias, int cols) {
    const size_t base = static_cast<size_t>(blockIdx.x) * cols;
    for (unsigned int c = threadIdx.x; c < static_cast<unsigned int>(cols); c += blockDim.x) {
        float x = __fadd_rn(y[base + c], bias[c]);
        if (Gelu) {
            // Preserve the CPU's left-associated 0.044715 * x * x * x.
            const float cubic = __fmul_rn(__fmul_rn(__fmul_rn(0.044715f, x), x), x);
            const float t = tanhf(__fmul_rn(0.7978845608f, __fadd_rn(x, cubic)));
            x = __fmul_rn(__fmul_rn(0.5f, x), __fadd_rn(1.0f, t));
        }
        y[base + c] = x;
    }
}

static __global__ void mkwe_residual_add_kernel(float * y, const float * x, int cols) {
    const size_t base = static_cast<size_t>(blockIdx.x) * cols;
    for (unsigned int c = threadIdx.x; c < static_cast<unsigned int>(cols); c += blockDim.x) {
        y[base + c] = __fadd_rn(y[base + c], x[base + c]);
    }
}

// In-place softmax on scores[head][query][key], with full f32 exp/sum/output.
static __global__ void mkwe_softmax_kernel(float * scores, int cols) {
    float * row = scores + static_cast<size_t>(blockIdx.x) * cols;
    __shared__ float maximum;
    __shared__ float inverse;
    float value = -CUDART_INF_F;
    for (unsigned int c = threadIdx.x; c < static_cast<unsigned int>(cols); c += blockDim.x) {
        value = fmaxf(value, row[c]);
    }
    value = mkwe_reduce<true>(value);
    if (threadIdx.x == 0) {
        maximum = value;
    }
    __syncthreads();

    float sum = 0.0f;
    for (unsigned int c = threadIdx.x; c < static_cast<unsigned int>(cols); c += blockDim.x) {
        const float probability = expf(row[c] - maximum);
        row[c] = probability;
        sum = __fadd_rn(sum, probability);
    }
    sum = mkwe_reduce<false>(sum);
    if (threadIdx.x == 0) {
        inverse = 1.0f / sum;
    }
    __syncthreads();
    for (unsigned int c = threadIdx.x; c < static_cast<unsigned int>(cols); c += blockDim.x) {
        row[c] = __fmul_rn(row[c], inverse);
    }
}

extern "C" cudaError_t mkwe_layer_norm(
        const float * x, const float * weight, const float * bias, float * y,
        int rows, int cols, cudaStream_t stream) {
    if (rows < 0 || cols <= 0) return cudaErrorInvalidValue;
    if (rows == 0) return cudaSuccess;
    mkwe_layer_norm_kernel<<<rows, 256, 0, stream>>>(x, weight, bias, y, cols);
    return cudaGetLastError();
}

extern "C" cudaError_t mkwe_bias_add(
        float * y, const float * bias, int rows, int cols, cudaStream_t stream) {
    if (rows < 0 || cols <= 0) return cudaErrorInvalidValue;
    if (rows == 0) return cudaSuccess;
    mkwe_bias_kernel<false><<<rows, 256, 0, stream>>>(y, bias, cols);
    return cudaGetLastError();
}

extern "C" cudaError_t mkwe_bias_gelu_tanh(
        float * y, const float * bias, int rows, int cols, cudaStream_t stream) {
    if (rows < 0 || cols <= 0) return cudaErrorInvalidValue;
    if (rows == 0) return cudaSuccess;
    mkwe_bias_kernel<true><<<rows, 256, 0, stream>>>(y, bias, cols);
    return cudaGetLastError();
}

extern "C" cudaError_t mkwe_residual_add(
        float * y, const float * x, int rows, int cols, cudaStream_t stream) {
    if (rows < 0 || cols <= 0) return cudaErrorInvalidValue;
    if (rows == 0) return cudaSuccess;
    mkwe_residual_add_kernel<<<rows, 256, 0, stream>>>(y, x, cols);
    return cudaGetLastError();
}

extern "C" cudaError_t mkwe_softmax(
        float * scores, int rows, int cols, cudaStream_t stream) {
    if (rows < 0 || cols <= 0) return cudaErrorInvalidValue;
    if (rows == 0) return cudaSuccess;
    mkwe_softmax_kernel<<<rows, 256, 0, stream>>>(scores, cols);
    return cudaGetLastError();
}
