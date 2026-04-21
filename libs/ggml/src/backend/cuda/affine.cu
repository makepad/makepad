#include <cuda_runtime.h>
#include <cuda_bf16.h>
#include <stdint.h>

static __device__ __forceinline__ float bf16_round_f32(const float value) {
    return __uint_as_float(__float_as_uint(value) & 0xFFFF0000u);
}

template <int BITS>
static __global__ void makepad_ggml_cuda_affine_qmv_kernel(
    const uint16_t * input_bf16_words,
    const uint32_t * packed_weights_u32,
    const uint16_t * scales_bf16_words,
    const uint16_t * biases_bf16_words,
    uint16_t * output_bf16_words,
    const uint32_t n_in,
    const uint32_t weight_words_per_row,
    const uint32_t qparams_per_row,
    const uint32_t out_rows
) {
    const uint32_t row = blockIdx.x;
    if (row >= out_rows) {
        return;
    }

    constexpr uint32_t pack_factor = 32 / BITS;
    constexpr uint32_t group_size = 64;
    constexpr uint32_t words_per_group = group_size / pack_factor;
    constexpr uint32_t mask = (1u << BITS) - 1u;

    __shared__ float partial[128];

    const uint32_t tid = threadIdx.x;
    const uint32_t weight_row_start = row * weight_words_per_row;
    const uint32_t qparam_row_start = row * qparams_per_row;

    float thread_total = 0.0f;
    for (uint32_t group = tid; group < qparams_per_row; group += blockDim.x) {
        const float scale = __bfloat162float(*reinterpret_cast<const __nv_bfloat16 *>(
            scales_bf16_words + qparam_row_start + group
        ));
        const float bias = __bfloat162float(*reinterpret_cast<const __nv_bfloat16 *>(
            biases_bf16_words + qparam_row_start + group
        ));

        const uint32_t group_start = weight_row_start + group * words_per_group;
        float group_sum = 0.0f;
        float group_accum = 0.0f;
        uint32_t x_index = group * group_size;

        #pragma unroll
        for (uint32_t word_offset = 0; word_offset < words_per_group; ++word_offset) {
            uint32_t packed = packed_weights_u32[group_start + word_offset];

            #pragma unroll
            for (uint32_t elem = 0; elem < pack_factor; ++elem) {
                if (x_index >= n_in) {
                    break;
                }

                const float q = static_cast<float>(packed & mask);
                const float x = __bfloat162float(*reinterpret_cast<const __nv_bfloat16 *>(
                    input_bf16_words + x_index
                ));
                group_sum = __fadd_rn(group_sum, x);
                group_accum = __fadd_rn(group_accum, __fmul_rn(x, q));
                ++x_index;
                if constexpr (BITS != 8) {
                    packed >>= BITS;
                }
            }
        }

        const float scaled = bf16_round_f32(__fmul_rn(scale, group_accum));
        const float biased = bf16_round_f32(__fmul_rn(bias, group_sum));
        thread_total = __fadd_rn(thread_total, __fadd_rn(scaled, biased));
    }

    partial[tid] = thread_total;
    __syncthreads();

    for (uint32_t stride = blockDim.x / 2; stride > 0; stride >>= 1) {
        if (tid < stride) {
            partial[tid] = __fadd_rn(partial[tid], partial[tid + stride]);
        }
        __syncthreads();
    }

    if (tid == 0) {
        const float rounded = bf16_round_f32(partial[0]);
        *reinterpret_cast<__nv_bfloat16 *>(output_bf16_words + row) = __float2bfloat16_rn(rounded);
    }
}

extern "C" cudaError_t makepad_ggml_cuda_affine_qmv_bf16(
    const uint16_t * input_bf16_words,
    const uint32_t * packed_weights_u32,
    const uint16_t * scales_bf16_words,
    const uint16_t * biases_bf16_words,
    uint16_t * output_bf16_words,
    uint32_t n_in,
    uint32_t weight_words_per_row,
    uint32_t qparams_per_row,
    uint32_t out_rows,
    uint32_t bits,
    cudaStream_t stream
) {
    dim3 block(128, 1, 1);
    dim3 grid(out_rows, 1, 1);

    switch (bits) {
        case 4:
            makepad_ggml_cuda_affine_qmv_kernel<4><<<grid, block, 0, stream>>>(
                input_bf16_words,
                packed_weights_u32,
                scales_bf16_words,
                biases_bf16_words,
                output_bf16_words,
                n_in,
                weight_words_per_row,
                qparams_per_row,
                out_rows
            );
            break;
        case 8:
            makepad_ggml_cuda_affine_qmv_kernel<8><<<grid, block, 0, stream>>>(
                input_bf16_words,
                packed_weights_u32,
                scales_bf16_words,
                biases_bf16_words,
                output_bf16_words,
                n_in,
                weight_words_per_row,
                qparams_per_row,
                out_rows
            );
            break;
        default:
            return cudaErrorInvalidValue;
    }

    return cudaGetLastError();
}
