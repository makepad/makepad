#include <cuda_runtime.h>
#include <math.h>
#include <stdint.h>

static __device__ __forceinline__ float makepad_ggml_cuda_silu(float x) {
    return x / (1.0f + expf(-x));
}

static __global__ void makepad_ggml_cuda_ssm_conv_f32_kernel(
        const float * __restrict__ src0,
        const float * __restrict__ src1,
        float * __restrict__ dst,
        uint32_t d_conv,
        uint32_t d_inner,
        uint32_t n_tokens,
        uint32_t src0_token_stride,
        uint32_t src0_seq_stride,
        uint32_t src1_inner_stride,
        uint32_t dst_token_stride,
        uint32_t dst_seq_stride,
        uint32_t apply_silu) {
    const uint32_t inner_idx = blockIdx.y * blockDim.x + threadIdx.x;
    const uint32_t seq_idx = blockIdx.x;
    if (inner_idx >= d_inner) {
        return;
    }

    const float * seq_src = src0 + seq_idx * src0_seq_stride + inner_idx;
    const float * weight = src1 + inner_idx * src1_inner_stride;
    float * seq_dst = dst + seq_idx * dst_seq_stride + inner_idx;

    for (uint32_t token_idx = 0; token_idx < n_tokens; token_idx++) {
        float sum = 0.0f;
        for (uint32_t k = 0; k < d_conv; k++) {
            sum += seq_src[(token_idx + k) * src0_token_stride] * weight[k];
        }
        seq_dst[token_idx * dst_token_stride] =
            apply_silu ? makepad_ggml_cuda_silu(sum) : sum;
    }
}

extern "C" cudaError_t makepad_ggml_cuda_ssm_conv_f32(
        const float * src0,
        const float * src1,
        float * dst,
        uint32_t d_conv,
        uint32_t d_inner,
        uint32_t n_tokens,
        uint32_t n_seqs,
        uint32_t src0_token_stride,
        uint32_t src0_seq_stride,
        uint32_t src1_inner_stride,
        uint32_t dst_token_stride,
        uint32_t dst_seq_stride,
        uint32_t apply_silu,
        cudaStream_t stream) {
    if (d_conv == 0 || d_inner == 0 || n_seqs == 0) {
        return cudaSuccess;
    }
    const uint32_t block = 128;
    const dim3 grid(n_seqs, (d_inner + block - 1) / block, 1);
    makepad_ggml_cuda_ssm_conv_f32_kernel<<<grid, block, 0, stream>>>(
        src0,
        src1,
        dst,
        d_conv,
        d_inner,
        n_tokens,
        src0_token_stride,
        src0_seq_stride,
        src1_inner_stride,
        dst_token_stride,
        dst_seq_stride,
        apply_silu);
    return cudaGetLastError();
}
