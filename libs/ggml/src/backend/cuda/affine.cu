#include <cuda_runtime.h>
#include <cuda_bf16.h>
#include <cuda_fp16.h>
#include <sm_61_intrinsics.h>
#include <stdint.h>

struct __align__(4) block_q8_1 {
    half d;
    half s;
    int8_t qs[32];
};

static_assert(sizeof(block_q8_1) == 36, "wrong q8_1 block size");

static __device__ __forceinline__ float bf16_round_f32(const float value) {
    return __uint_as_float(__float_as_uint(value) & 0xFFFF0000u);
}

template <typename T>
static __device__ __forceinline__ T makepad_ggml_cuda_warp_reduce_sum(T value) {
    for (int offset = warpSize / 2; offset > 0; offset >>= 1) {
        value += __shfl_down_sync(0xffffffffu, value, offset);
    }
    return value;
}

template <typename T>
static __device__ __forceinline__ T makepad_ggml_cuda_block_reduce_sum(T value) {
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

template <int MAX_SLOTS>
static __device__ __forceinline__ void makepad_ggml_cuda_block_reduce_sum_slots(
    float (&values)[MAX_SLOTS]
) {
    __shared__ float shared[MAX_SLOTS * 32];
    const int lane = threadIdx.x & 31;
    const int warp = threadIdx.x >> 5;
    const int warp_count = (blockDim.x + 31) / 32;

    #pragma unroll
    for (int slot = 0; slot < MAX_SLOTS; ++slot) {
        values[slot] = makepad_ggml_cuda_warp_reduce_sum(values[slot]);
        if (lane == 0) {
            shared[slot * 32 + warp] = values[slot];
        }
    }
    __syncthreads();

    if (warp == 0) {
        #pragma unroll
        for (int slot = 0; slot < MAX_SLOTS; ++slot) {
            values[slot] = lane < warp_count ? shared[slot * 32 + lane] : 0.0f;
            values[slot] = makepad_ggml_cuda_warp_reduce_sum(values[slot]);
        }
    }
}

static __device__ __forceinline__ int makepad_ggml_cuda_dp4a_i8(
    const int a,
    const int b,
    const int c
) {
#if defined(__CUDA_ARCH__) && (__CUDA_ARCH__ >= 610)
    return __dp4a(a, b, c);
#else
    const int8_t * a8 = reinterpret_cast<const int8_t *>(&a);
    const int8_t * b8 = reinterpret_cast<const int8_t *>(&b);
    return c
        + static_cast<int>(a8[0]) * static_cast<int>(b8[0])
        + static_cast<int>(a8[1]) * static_cast<int>(b8[1])
        + static_cast<int>(a8[2]) * static_cast<int>(b8[2])
        + static_cast<int>(a8[3]) * static_cast<int>(b8[3]);
#endif
}

static __device__ __forceinline__ int makepad_ggml_cuda_center_q8_bytes(const uint32_t packed) {
    return static_cast<int>(packed ^ 0x80808080u);
}

static __device__ __forceinline__ int makepad_ggml_cuda_center_q4_bytes(const uint32_t packed) {
    return static_cast<int>(__vsub4(packed, 0x08080808u));
}

static __device__ __forceinline__ void makepad_ggml_cuda_affine_accum_q4_word(
    const uint32_t packed,
    const float2 x01,
    const float2 x23,
    const float2 x45,
    const float2 x67,
    float & group_accum
) {
    const float q0 = static_cast<float>(packed & 0x0Fu);
    const float q1 = static_cast<float>((packed >> 4) & 0x0Fu);
    const float q2 = static_cast<float>((packed >> 8) & 0x0Fu);
    const float q3 = static_cast<float>((packed >> 12) & 0x0Fu);
    const float q4 = static_cast<float>((packed >> 16) & 0x0Fu);
    const float q5 = static_cast<float>((packed >> 20) & 0x0Fu);
    const float q6 = static_cast<float>((packed >> 24) & 0x0Fu);
    const float q7 = static_cast<float>((packed >> 28) & 0x0Fu);

    group_accum = __fadd_rn(group_accum, __fmul_rn(x01.x, q0));
    group_accum = __fadd_rn(group_accum, __fmul_rn(x01.y, q1));
    group_accum = __fadd_rn(group_accum, __fmul_rn(x23.x, q2));
    group_accum = __fadd_rn(group_accum, __fmul_rn(x23.y, q3));
    group_accum = __fadd_rn(group_accum, __fmul_rn(x45.x, q4));
    group_accum = __fadd_rn(group_accum, __fmul_rn(x45.y, q5));
    group_accum = __fadd_rn(group_accum, __fmul_rn(x67.x, q6));
    group_accum = __fadd_rn(group_accum, __fmul_rn(x67.y, q7));
}

static __device__ __forceinline__ void makepad_ggml_cuda_affine_accum_q8_word(
    const uint32_t packed,
    const float2 x01,
    const float2 x23,
    float & group_sum,
    float & group_accum
) {
    const float q0 = static_cast<float>(packed & 0xFFu);
    const float q1 = static_cast<float>((packed >> 8) & 0xFFu);
    const float q2 = static_cast<float>((packed >> 16) & 0xFFu);
    const float q3 = static_cast<float>((packed >> 24) & 0xFFu);

    group_sum = __fadd_rn(group_sum, x01.x);
    group_accum = __fadd_rn(group_accum, __fmul_rn(x01.x, q0));
    group_sum = __fadd_rn(group_sum, x01.y);
    group_accum = __fadd_rn(group_accum, __fmul_rn(x01.y, q1));
    group_sum = __fadd_rn(group_sum, x23.x);
    group_accum = __fadd_rn(group_accum, __fmul_rn(x23.x, q2));
    group_sum = __fadd_rn(group_sum, x23.y);
    group_accum = __fadd_rn(group_accum, __fmul_rn(x23.y, q3));
}

static inline uint32_t makepad_ggml_cuda_affine_block_size(const uint32_t qparams_per_row) {
    if (qparams_per_row <= 32) {
        return 32;
    }
    if (qparams_per_row <= 64) {
        return 64;
    }
    return 128;
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
                packed >>= BITS;
            }
        }

        const float scaled = bf16_round_f32(__fmul_rn(scale, group_accum));
        const float biased = bf16_round_f32(__fmul_rn(bias, group_sum));
        thread_total = __fadd_rn(thread_total, __fadd_rn(scaled, biased));
    }
    thread_total = makepad_ggml_cuda_block_reduce_sum(thread_total);

    if (tid == 0) {
        const float rounded = bf16_round_f32(thread_total);
        *reinterpret_cast<__nv_bfloat16 *>(output_bf16_words + row) = __float2bfloat16_rn(rounded);
    }
}

template <int BITS>
static __global__ void makepad_ggml_cuda_affine_qmv_f32_kernel(
    const uint16_t * input_bf16_words,
    const uint32_t * packed_weights_u32,
    const uint16_t * scales_bf16_words,
    const uint16_t * biases_bf16_words,
    float * output_f32,
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
                packed >>= BITS;
            }
        }

        const float scaled = bf16_round_f32(__fmul_rn(scale, group_accum));
        const float biased = bf16_round_f32(__fmul_rn(bias, group_sum));
        thread_total = __fadd_rn(thread_total, __fadd_rn(scaled, biased));
    }
    thread_total = makepad_ggml_cuda_block_reduce_sum(thread_total);

    if (tid == 0) {
        output_f32[row] = bf16_round_f32(thread_total);
    }
}

template <int BITS>
static __global__ void makepad_ggml_cuda_affine_qmv_f32_precise_kernel(
    const uint16_t * __restrict__ input_bf16_words,
    const uint32_t * __restrict__ packed_weights_u32,
    const uint16_t * __restrict__ scales_bf16_words,
    const uint16_t * __restrict__ biases_bf16_words,
    float * __restrict__ output_f32,
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

        if constexpr (BITS == 8) {
            #pragma unroll
            for (uint32_t word_offset = 0; word_offset < words_per_group;) {
                if (word_offset + 1 < words_per_group && x_index + 7 < n_in) {
                    const uint32_t packed0 = packed_weights_u32[group_start + word_offset + 0];
                    const uint32_t packed1 = packed_weights_u32[group_start + word_offset + 1];
                    const float2 x01 = __bfloat1622float2(
                        *reinterpret_cast<const __nv_bfloat162 *>(input_bf16_words + x_index)
                    );
                    const float2 x23 = __bfloat1622float2(
                        *reinterpret_cast<const __nv_bfloat162 *>(input_bf16_words + x_index + 2)
                    );
                    const float2 x45 = __bfloat1622float2(
                        *reinterpret_cast<const __nv_bfloat162 *>(input_bf16_words + x_index + 4)
                    );
                    const float2 x67 = __bfloat1622float2(
                        *reinterpret_cast<const __nv_bfloat162 *>(input_bf16_words + x_index + 6)
                    );
                    makepad_ggml_cuda_affine_accum_q8_word(
                        packed0,
                        x01,
                        x23,
                        group_sum,
                        group_accum
                    );
                    makepad_ggml_cuda_affine_accum_q8_word(
                        packed1,
                        x45,
                        x67,
                        group_sum,
                        group_accum
                    );
                    x_index += 8;
                    word_offset += 2;
                } else {
                    const uint32_t packed = packed_weights_u32[group_start + word_offset];
                    ++word_offset;
                    if (x_index + 3 < n_in) {
                        const float2 x01 = __bfloat1622float2(
                            *reinterpret_cast<const __nv_bfloat162 *>(input_bf16_words + x_index)
                        );
                        const float2 x23 = __bfloat1622float2(
                            *reinterpret_cast<const __nv_bfloat162 *>(input_bf16_words + x_index + 2)
                        );
                        makepad_ggml_cuda_affine_accum_q8_word(
                            packed,
                            x01,
                            x23,
                            group_sum,
                            group_accum
                        );
                        x_index += 4;
                    } else {
                    uint32_t tail = packed;
                    #pragma unroll
                    for (uint32_t elem = 0; elem < pack_factor; ++elem) {
                        if (x_index >= n_in) {
                            break;
                        }
                        const float q = static_cast<float>(tail & mask);
                        const float x = __bfloat162float(*reinterpret_cast<const __nv_bfloat16 *>(
                            input_bf16_words + x_index
                        ));
                        group_sum = __fadd_rn(group_sum, x);
                        group_accum = __fadd_rn(group_accum, __fmul_rn(x, q));
                        ++x_index;
                        tail >>= BITS;
                    }
                    }
                }
            }
        } else {
            #pragma unroll
            for (uint32_t word_offset = 0; word_offset < words_per_group; ++word_offset) {
                const uint32_t packed = packed_weights_u32[group_start + word_offset];
                if (x_index + 7 < n_in) {
                    const float2 x01 = __bfloat1622float2(
                        *reinterpret_cast<const __nv_bfloat162 *>(input_bf16_words + x_index)
                    );
                    const float2 x23 = __bfloat1622float2(
                        *reinterpret_cast<const __nv_bfloat162 *>(input_bf16_words + x_index + 2)
                    );
                    const float2 x45 = __bfloat1622float2(
                        *reinterpret_cast<const __nv_bfloat162 *>(input_bf16_words + x_index + 4)
                    );
                    const float2 x67 = __bfloat1622float2(
                        *reinterpret_cast<const __nv_bfloat162 *>(input_bf16_words + x_index + 6)
                    );
                    group_sum = __fadd_rn(group_sum, x01.x);
                    group_sum = __fadd_rn(group_sum, x01.y);
                    group_sum = __fadd_rn(group_sum, x23.x);
                    group_sum = __fadd_rn(group_sum, x23.y);
                    group_sum = __fadd_rn(group_sum, x45.x);
                    group_sum = __fadd_rn(group_sum, x45.y);
                    group_sum = __fadd_rn(group_sum, x67.x);
                    group_sum = __fadd_rn(group_sum, x67.y);
                    makepad_ggml_cuda_affine_accum_q4_word(
                        packed,
                        x01,
                        x23,
                        x45,
                        x67,
                        group_accum
                    );
                    x_index += 8;
                } else {
                    uint32_t tail = packed;
                    #pragma unroll
                    for (uint32_t elem = 0; elem < pack_factor; ++elem) {
                        if (x_index >= n_in) {
                            break;
                        }

                        const float q = static_cast<float>(tail & mask);
                        const float x = __bfloat162float(*reinterpret_cast<const __nv_bfloat16 *>(
                            input_bf16_words + x_index
                        ));
                        group_sum = __fadd_rn(group_sum, x);
                        group_accum = __fadd_rn(group_accum, __fmul_rn(x, q));
                        ++x_index;
                        tail >>= BITS;
                    }
                }
            }
        }

        thread_total = __fadd_rn(
            thread_total,
            __fadd_rn(__fmul_rn(scale, group_accum), __fmul_rn(bias, group_sum))
        );
    }
    thread_total = makepad_ggml_cuda_block_reduce_sum(thread_total);

    if (tid == 0) {
        output_f32[row] = thread_total;
    }
}

template <int BITS, int ROW_TILE>
static __global__ void makepad_ggml_cuda_affine_qmv_f32_rows_precise_kernel(
    const uint16_t * __restrict__ input_bf16_words,
    const uint32_t * __restrict__ packed_weights_u32,
    const uint16_t * __restrict__ scales_bf16_words,
    const uint16_t * __restrict__ biases_bf16_words,
    float * __restrict__ output_f32,
    const uint32_t n_in,
    const uint32_t weight_words_per_row,
    const uint32_t qparams_per_row,
    const uint32_t out_rows,
    const uint32_t input_rows
) {
    const uint32_t row = blockIdx.x;
    if (row >= out_rows) {
        return;
    }
    const uint32_t batch_base = blockIdx.y * ROW_TILE;

    constexpr uint32_t pack_factor = 32 / BITS;
    constexpr uint32_t group_size = 64;
    constexpr uint32_t words_per_group = group_size / pack_factor;
    constexpr uint32_t mask = (1u << BITS) - 1u;

    const uint32_t tid = threadIdx.x;
    const uint32_t weight_row_start = row * weight_words_per_row;
    const uint32_t qparam_row_start = row * qparams_per_row;
    float thread_total[ROW_TILE];
    #pragma unroll
    for (int slot = 0; slot < ROW_TILE; ++slot) {
        thread_total[slot] = 0.0f;
    }
    for (uint32_t group = tid; group < qparams_per_row; group += blockDim.x) {
        const float scale = __bfloat162float(*reinterpret_cast<const __nv_bfloat16 *>(
            scales_bf16_words + qparam_row_start + group
        ));
        const float bias = __bfloat162float(*reinterpret_cast<const __nv_bfloat16 *>(
            biases_bf16_words + qparam_row_start + group
        ));

        const uint32_t group_start = weight_row_start + group * words_per_group;
        float group_sum[ROW_TILE];
        float group_accum[ROW_TILE];
        #pragma unroll
        for (int slot = 0; slot < ROW_TILE; ++slot) {
            group_sum[slot] = 0.0f;
            group_accum[slot] = 0.0f;
        }
        uint32_t x_index = group * group_size;

        if constexpr (BITS == 8) {
            #pragma unroll
            for (uint32_t word_offset = 0; word_offset < words_per_group;) {
                if (word_offset + 1 < words_per_group && x_index + 7 < n_in) {
                    const uint32_t packed0 = packed_weights_u32[group_start + word_offset + 0];
                    const uint32_t packed1 = packed_weights_u32[group_start + word_offset + 1];
                    #pragma unroll
                    for (int slot = 0; slot < ROW_TILE; ++slot) {
                        const uint32_t batch = batch_base + slot;
                        if (batch >= input_rows) {
                            continue;
                        }
                        const uint16_t * batch_input = input_bf16_words + batch * n_in;
                        const float2 x01 = __bfloat1622float2(
                            *reinterpret_cast<const __nv_bfloat162 *>(batch_input + x_index)
                        );
                        const float2 x23 = __bfloat1622float2(
                            *reinterpret_cast<const __nv_bfloat162 *>(batch_input + x_index + 2)
                        );
                        const float2 x45 = __bfloat1622float2(
                            *reinterpret_cast<const __nv_bfloat162 *>(batch_input + x_index + 4)
                        );
                        const float2 x67 = __bfloat1622float2(
                            *reinterpret_cast<const __nv_bfloat162 *>(batch_input + x_index + 6)
                        );
                        makepad_ggml_cuda_affine_accum_q8_word(
                            packed0,
                            x01,
                            x23,
                            group_sum[slot],
                            group_accum[slot]
                        );
                        makepad_ggml_cuda_affine_accum_q8_word(
                            packed1,
                            x45,
                            x67,
                            group_sum[slot],
                            group_accum[slot]
                        );
                    }
                    x_index += 8;
                    word_offset += 2;
                } else {
                    const uint32_t packed = packed_weights_u32[group_start + word_offset];
                    ++word_offset;
                    if (x_index + 3 < n_in) {
                        #pragma unroll
                        for (int slot = 0; slot < ROW_TILE; ++slot) {
                            const uint32_t batch = batch_base + slot;
                            if (batch >= input_rows) {
                                continue;
                            }
                            const uint16_t * batch_input = input_bf16_words + batch * n_in;
                            const float2 x01 = __bfloat1622float2(
                                *reinterpret_cast<const __nv_bfloat162 *>(batch_input + x_index)
                            );
                            const float2 x23 = __bfloat1622float2(
                                *reinterpret_cast<const __nv_bfloat162 *>(batch_input + x_index + 2)
                            );
                            makepad_ggml_cuda_affine_accum_q8_word(
                                packed,
                                x01,
                                x23,
                                group_sum[slot],
                                group_accum[slot]
                            );
                        }
                        x_index += 4;
                    } else {
                        uint32_t tail = packed;
                        #pragma unroll
                        for (uint32_t elem = 0; elem < pack_factor; ++elem) {
                            if (x_index >= n_in) {
                                break;
                            }
                            const float q = static_cast<float>(tail & mask);
                            #pragma unroll
                            for (int slot = 0; slot < ROW_TILE; ++slot) {
                                const uint32_t batch = batch_base + slot;
                                if (batch >= input_rows) {
                                    continue;
                                }
                                const uint16_t * batch_input = input_bf16_words + batch * n_in;
                                const float x = __bfloat162float(*reinterpret_cast<const __nv_bfloat16 *>(
                                    batch_input + x_index
                                ));
                                group_sum[slot] = __fadd_rn(group_sum[slot], x);
                                group_accum[slot] = __fadd_rn(group_accum[slot], __fmul_rn(x, q));
                            }
                            ++x_index;
                            tail >>= BITS;
                        }
                    }
                }
            }
        } else {
            #pragma unroll
            for (uint32_t word_offset = 0; word_offset < words_per_group; ++word_offset) {
                const uint32_t packed = packed_weights_u32[group_start + word_offset];
                if (x_index + 7 < n_in) {
                    #pragma unroll
                    for (int slot = 0; slot < ROW_TILE; ++slot) {
                        const uint32_t batch = batch_base + slot;
                        if (batch >= input_rows) {
                            continue;
                        }
                        const uint16_t * batch_input = input_bf16_words + batch * n_in;
                        const float2 x01 = __bfloat1622float2(
                            *reinterpret_cast<const __nv_bfloat162 *>(batch_input + x_index)
                        );
                        const float2 x23 = __bfloat1622float2(
                            *reinterpret_cast<const __nv_bfloat162 *>(batch_input + x_index + 2)
                        );
                        const float2 x45 = __bfloat1622float2(
                            *reinterpret_cast<const __nv_bfloat162 *>(batch_input + x_index + 4)
                        );
                        const float2 x67 = __bfloat1622float2(
                            *reinterpret_cast<const __nv_bfloat162 *>(batch_input + x_index + 6)
                        );
                        group_sum[slot] = __fadd_rn(group_sum[slot], x01.x);
                        group_sum[slot] = __fadd_rn(group_sum[slot], x01.y);
                        group_sum[slot] = __fadd_rn(group_sum[slot], x23.x);
                        group_sum[slot] = __fadd_rn(group_sum[slot], x23.y);
                        group_sum[slot] = __fadd_rn(group_sum[slot], x45.x);
                        group_sum[slot] = __fadd_rn(group_sum[slot], x45.y);
                        group_sum[slot] = __fadd_rn(group_sum[slot], x67.x);
                        group_sum[slot] = __fadd_rn(group_sum[slot], x67.y);
                        makepad_ggml_cuda_affine_accum_q4_word(
                            packed,
                            x01,
                            x23,
                            x45,
                            x67,
                            group_accum[slot]
                        );
                    }
                    x_index += 8;
                } else {
                    uint32_t tail = packed;
                    #pragma unroll
                    for (uint32_t elem = 0; elem < pack_factor; ++elem) {
                        if (x_index >= n_in) {
                            break;
                        }

                        const float q = static_cast<float>(tail & mask);
                        #pragma unroll
                        for (int slot = 0; slot < ROW_TILE; ++slot) {
                            const uint32_t batch = batch_base + slot;
                            if (batch >= input_rows) {
                                continue;
                            }
                            const uint16_t * batch_input = input_bf16_words + batch * n_in;
                            const float x = __bfloat162float(*reinterpret_cast<const __nv_bfloat16 *>(
                                batch_input + x_index
                            ));
                            group_sum[slot] = __fadd_rn(group_sum[slot], x);
                            group_accum[slot] = __fadd_rn(group_accum[slot], __fmul_rn(x, q));
                        }
                        ++x_index;
                        tail >>= BITS;
                    }
                }
            }
        }

        #pragma unroll
        for (int slot = 0; slot < ROW_TILE; ++slot) {
            if (batch_base + slot >= input_rows) {
                continue;
            }
            thread_total[slot] = __fadd_rn(
                thread_total[slot],
                __fadd_rn(__fmul_rn(scale, group_accum[slot]), __fmul_rn(bias, group_sum[slot]))
            );
        }
    }
    makepad_ggml_cuda_block_reduce_sum_slots<ROW_TILE>(thread_total);

    if (tid == 0) {
        #pragma unroll
        for (int slot = 0; slot < ROW_TILE; ++slot) {
            const uint32_t batch = batch_base + slot;
            if (batch < input_rows) {
                output_f32[batch * out_rows + row] = thread_total[slot];
            }
        }
    }
}

template <int BITS>
static __global__ void makepad_ggml_cuda_affine_q8_1_qmv_f32_precise_kernel(
    const uint16_t * __restrict__ input_bf16_words,
    const block_q8_1 * __restrict__ input_q8_1,
    const uint32_t * __restrict__ packed_weights_u32,
    const uint16_t * __restrict__ scales_bf16_words,
    const uint16_t * __restrict__ biases_bf16_words,
    float * __restrict__ output_f32,
    const uint32_t weight_words_per_row,
    const uint32_t qparams_per_row,
    const uint32_t out_rows
) {
    const uint32_t row = blockIdx.x;
    if (row >= out_rows) {
        return;
    }

    constexpr uint32_t group_size = 64;
    constexpr uint32_t q8_blocks_per_group = group_size / 32;
    constexpr uint32_t q8_words_per_block = 32 / 4;
    constexpr uint32_t q8_words_per_group = q8_blocks_per_group * q8_words_per_block;
    constexpr uint32_t weight_words_per_group = BITS == 8 ? q8_words_per_group : (q8_words_per_group / 2);
    constexpr float zero_point = BITS == 8 ? 128.0f : 8.0f;

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
        const uint32_t group_start = weight_row_start + group * weight_words_per_group;
        const block_q8_1 * input_group = input_q8_1 + group * q8_blocks_per_group;

        float group_sum = 0.0f;
        const uint32_t input_group_start = group * group_size;
        #pragma unroll
        for (uint32_t elem = 0; elem < group_size; ++elem) {
            group_sum = __fadd_rn(
                group_sum,
                __bfloat162float(*reinterpret_cast<const __nv_bfloat16 *>(
                    input_bf16_words + input_group_start + elem
                ))
            );
        }
        float group_accum = 0.0f;

        if constexpr (BITS == 8) {
            #pragma unroll
            for (uint32_t q8_block = 0; q8_block < q8_blocks_per_group; ++q8_block) {
                const block_q8_1 * block = input_group + q8_block;
                const int * input_words = reinterpret_cast<const int *>(block->qs);
                const float input_scale = __half2float(block->d);
                int dot = 0;
                #pragma unroll
                for (uint32_t word = 0; word < q8_words_per_block; ++word) {
                    const int centered = makepad_ggml_cuda_center_q8_bytes(
                        packed_weights_u32[group_start + q8_block * q8_words_per_block + word]
                    );
                    dot = makepad_ggml_cuda_dp4a_i8(centered, input_words[word], dot);
                }
                group_accum = __fadd_rn(group_accum, __fmul_rn(input_scale, static_cast<float>(dot)));
            }
        } else {
            constexpr uint32_t q4_words_per_group = weight_words_per_group;
            constexpr uint32_t q4_words_per_block = q4_words_per_group / q8_blocks_per_group;
            #pragma unroll
            for (uint32_t q8_block = 0; q8_block < q8_blocks_per_group; ++q8_block) {
                const block_q8_1 * block = input_group + q8_block;
                const int * input_words = reinterpret_cast<const int *>(block->qs);
                const float input_scale = __half2float(block->d);
                int dot = 0;
                #pragma unroll
                for (uint32_t word = 0; word < q4_words_per_block; ++word) {
                    const uint32_t packed =
                        packed_weights_u32[group_start + q8_block * q4_words_per_block + word];
                    const int low = makepad_ggml_cuda_center_q4_bytes(packed & 0x0F0F0F0Fu);
                    const int high =
                        makepad_ggml_cuda_center_q4_bytes((packed >> 4) & 0x0F0F0F0Fu);
                    dot = makepad_ggml_cuda_dp4a_i8(low, input_words[word * 2 + 0], dot);
                    dot = makepad_ggml_cuda_dp4a_i8(high, input_words[word * 2 + 1], dot);
                }
                group_accum = __fadd_rn(group_accum, __fmul_rn(input_scale, static_cast<float>(dot)));
            }
        }

        thread_total = __fadd_rn(
            thread_total,
            __fadd_rn(
                __fmul_rn(scale, group_accum),
                __fmul_rn(__fadd_rn(bias, __fmul_rn(zero_point, scale)), group_sum)
            )
        );
    }

    thread_total = makepad_ggml_cuda_block_reduce_sum(thread_total);
    if (tid == 0) {
        output_f32[row] = thread_total;
    }
}

template <int BITS>
static __global__ void makepad_ggml_cuda_affine_qmv_f32_select_plane_precise_kernel(
    const uint16_t * __restrict__ input_bf16_words,
    const uint32_t * __restrict__ packed_weights_u32,
    const uint16_t * __restrict__ scales_bf16_words,
    const uint16_t * __restrict__ biases_bf16_words,
    const uint32_t * __restrict__ plane_indices_u32,
    const uint32_t plane_slot,
    float * __restrict__ output_f32,
    const uint32_t n_in,
    const uint32_t weight_words_per_row,
    const uint32_t qparams_per_row,
    const uint32_t out_rows,
    const uint32_t weight_words_per_plane,
    const uint32_t qparams_words_per_plane,
    const uint32_t plane_count
) {
    const uint32_t row = blockIdx.x;
    if (row >= out_rows) {
        return;
    }

    const uint32_t plane = plane_indices_u32[plane_slot];
    if (plane >= plane_count) {
        if (threadIdx.x == 0) {
            output_f32[row] = 0.0f;
        }
        return;
    }

    constexpr uint32_t pack_factor = 32 / BITS;
    constexpr uint32_t group_size = 64;
    constexpr uint32_t words_per_group = group_size / pack_factor;
    constexpr uint32_t mask = (1u << BITS) - 1u;

    const uint32_t tid = threadIdx.x;
    const uint32_t weight_row_start =
        plane * weight_words_per_plane + row * weight_words_per_row;
    const uint32_t qparam_row_start =
        plane * qparams_words_per_plane + row * qparams_per_row;

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
            const uint32_t packed = packed_weights_u32[group_start + word_offset];
            if constexpr (BITS == 4) {
                if (x_index + 7 < n_in) {
                    const float2 x01 = __bfloat1622float2(
                        *reinterpret_cast<const __nv_bfloat162 *>(input_bf16_words + x_index)
                    );
                    const float2 x23 = __bfloat1622float2(
                        *reinterpret_cast<const __nv_bfloat162 *>(input_bf16_words + x_index + 2)
                    );
                    const float2 x45 = __bfloat1622float2(
                        *reinterpret_cast<const __nv_bfloat162 *>(input_bf16_words + x_index + 4)
                    );
                    const float2 x67 = __bfloat1622float2(
                        *reinterpret_cast<const __nv_bfloat162 *>(input_bf16_words + x_index + 6)
                    );
                    group_sum = __fadd_rn(group_sum, x01.x);
                    group_sum = __fadd_rn(group_sum, x01.y);
                    group_sum = __fadd_rn(group_sum, x23.x);
                    group_sum = __fadd_rn(group_sum, x23.y);
                    group_sum = __fadd_rn(group_sum, x45.x);
                    group_sum = __fadd_rn(group_sum, x45.y);
                    group_sum = __fadd_rn(group_sum, x67.x);
                    group_sum = __fadd_rn(group_sum, x67.y);
                    makepad_ggml_cuda_affine_accum_q4_word(
                        packed,
                        x01,
                        x23,
                        x45,
                        x67,
                        group_accum
                    );
                    x_index += 8;
                } else {
                    uint32_t tail = packed;
                    #pragma unroll
                    for (uint32_t elem = 0; elem < pack_factor; ++elem) {
                        if (x_index >= n_in) {
                            break;
                        }

                        const float q = static_cast<float>(tail & mask);
                        const float x = __bfloat162float(*reinterpret_cast<const __nv_bfloat16 *>(
                            input_bf16_words + x_index
                        ));
                        group_sum = __fadd_rn(group_sum, x);
                        group_accum = __fadd_rn(group_accum, __fmul_rn(x, q));
                        ++x_index;
                        tail >>= BITS;
                    }
                }
            } else {
                uint32_t tail = packed;
                #pragma unroll
                for (uint32_t elem = 0; elem < pack_factor; ++elem) {
                    if (x_index >= n_in) {
                        break;
                    }

                    const float q = static_cast<float>(tail & mask);
                    const float x = __bfloat162float(*reinterpret_cast<const __nv_bfloat16 *>(
                        input_bf16_words + x_index
                    ));
                    group_sum = __fadd_rn(group_sum, x);
                    group_accum = __fadd_rn(group_accum, __fmul_rn(x, q));
                    ++x_index;
                    tail >>= BITS;
                }
            }
        }

        thread_total = __fadd_rn(
            thread_total,
            __fadd_rn(__fmul_rn(scale, group_accum), __fmul_rn(bias, group_sum))
        );
    }

    thread_total = makepad_ggml_cuda_block_reduce_sum(thread_total);

    if (tid == 0) {
        output_f32[row] = thread_total;
    }
}

template <int BITS>
static __global__ void makepad_ggml_cuda_affine_qmv_f32_select_plane_rows_precise_kernel(
    const uint16_t * __restrict__ input_bf16_words,
    const uint32_t * __restrict__ packed_weights_u32,
    const uint16_t * __restrict__ scales_bf16_words,
    const uint16_t * __restrict__ biases_bf16_words,
    const uint32_t * __restrict__ plane_indices_u32,
    const uint32_t plane_indices_row_stride,
    const uint32_t plane_slot,
    float * __restrict__ output_f32,
    const uint32_t n_in,
    const uint32_t weight_words_per_row,
    const uint32_t qparams_per_row,
    const uint32_t out_rows,
    const uint32_t input_rows,
    const uint32_t weight_words_per_plane,
    const uint32_t qparams_words_per_plane,
    const uint32_t plane_count
) {
    const uint32_t row = blockIdx.x;
    const uint32_t batch = blockIdx.y;
    if (row >= out_rows || batch >= input_rows) {
        return;
    }

    constexpr uint32_t pack_factor = 32 / BITS;
    constexpr uint32_t group_size = 64;
    constexpr uint32_t words_per_group = group_size / pack_factor;
    constexpr uint32_t mask = (1u << BITS) - 1u;

    const uint32_t tid = threadIdx.x;
    const uint32_t plane = plane_indices_u32[batch * plane_indices_row_stride + plane_slot];
    if (plane >= plane_count) {
        if (tid == 0) {
            output_f32[batch * out_rows + row] = 0.0f;
        }
        return;
    }
    const uint16_t * batch_input = input_bf16_words + batch * n_in;
    const uint32_t weight_row_start =
        plane * weight_words_per_plane + row * weight_words_per_row;
    const uint32_t qparam_row_start =
        plane * qparams_words_per_plane + row * qparams_per_row;
    float thread_total = 0.0f;

    for (uint32_t group = tid; group < qparams_per_row; group += blockDim.x) {
        const float scale = __bfloat162float(*reinterpret_cast<const __nv_bfloat16 *>(
            scales_bf16_words + qparam_row_start + group
        ));
        const float bias = __bfloat162float(*reinterpret_cast<const __nv_bfloat16 *>(
            biases_bf16_words + qparam_row_start + group
        ));

        const uint32_t group_start = weight_row_start + group * words_per_group;
        uint32_t x_index = group * group_size;
        float group_sum = 0.0f;
        float group_accum = 0.0f;

        if constexpr (BITS == 8) {
            #pragma unroll
            for (uint32_t word_offset = 0; word_offset < words_per_group;) {
                if (word_offset + 1 < words_per_group && x_index + 7 < n_in) {
                    const uint32_t packed0 = packed_weights_u32[group_start + word_offset + 0];
                    const uint32_t packed1 = packed_weights_u32[group_start + word_offset + 1];
                    const float2 x01 = __bfloat1622float2(
                        *reinterpret_cast<const __nv_bfloat162 *>(batch_input + x_index)
                    );
                    const float2 x23 = __bfloat1622float2(
                        *reinterpret_cast<const __nv_bfloat162 *>(batch_input + x_index + 2)
                    );
                    const float2 x45 = __bfloat1622float2(
                        *reinterpret_cast<const __nv_bfloat162 *>(batch_input + x_index + 4)
                    );
                    const float2 x67 = __bfloat1622float2(
                        *reinterpret_cast<const __nv_bfloat162 *>(batch_input + x_index + 6)
                    );
                    makepad_ggml_cuda_affine_accum_q8_word(
                        packed0,
                        x01,
                        x23,
                        group_sum,
                        group_accum
                    );
                    makepad_ggml_cuda_affine_accum_q8_word(
                        packed1,
                        x45,
                        x67,
                        group_sum,
                        group_accum
                    );
                    x_index += 8;
                    word_offset += 2;
                } else {
                    uint32_t packed = packed_weights_u32[group_start + word_offset];
                    ++word_offset;
                    if (x_index + 3 < n_in) {
                        const float2 x01 = __bfloat1622float2(
                            *reinterpret_cast<const __nv_bfloat162 *>(batch_input + x_index)
                        );
                        const float2 x23 = __bfloat1622float2(
                            *reinterpret_cast<const __nv_bfloat162 *>(batch_input + x_index + 2)
                        );
                        makepad_ggml_cuda_affine_accum_q8_word(
                            packed,
                            x01,
                            x23,
                            group_sum,
                            group_accum
                        );
                        x_index += 4;
                    } else {
                        #pragma unroll
                        for (uint32_t elem = 0; elem < pack_factor; ++elem) {
                            if (x_index >= n_in) {
                                break;
                            }
                            const float x = __bfloat162float(*reinterpret_cast<const __nv_bfloat16 *>(
                                batch_input + x_index
                            ));
                            group_sum = __fadd_rn(group_sum, x);
                            group_accum = __fadd_rn(
                                group_accum,
                                __fmul_rn(x, static_cast<float>(packed & mask))
                            );
                            packed >>= BITS;
                            ++x_index;
                        }
                    }
                }
            }
        } else {
            #pragma unroll
            for (uint32_t word_offset = 0; word_offset < words_per_group; ++word_offset) {
                uint32_t packed = packed_weights_u32[group_start + word_offset];
                if (x_index + 7 < n_in) {
                    const float2 x01 = __bfloat1622float2(
                        *reinterpret_cast<const __nv_bfloat162 *>(batch_input + x_index)
                    );
                    const float2 x23 = __bfloat1622float2(
                        *reinterpret_cast<const __nv_bfloat162 *>(batch_input + x_index + 2)
                    );
                    const float2 x45 = __bfloat1622float2(
                        *reinterpret_cast<const __nv_bfloat162 *>(batch_input + x_index + 4)
                    );
                    const float2 x67 = __bfloat1622float2(
                        *reinterpret_cast<const __nv_bfloat162 *>(batch_input + x_index + 6)
                    );
                    group_sum = __fadd_rn(group_sum, x01.x);
                    group_sum = __fadd_rn(group_sum, x01.y);
                    group_sum = __fadd_rn(group_sum, x23.x);
                    group_sum = __fadd_rn(group_sum, x23.y);
                    group_sum = __fadd_rn(group_sum, x45.x);
                    group_sum = __fadd_rn(group_sum, x45.y);
                    group_sum = __fadd_rn(group_sum, x67.x);
                    group_sum = __fadd_rn(group_sum, x67.y);
                    makepad_ggml_cuda_affine_accum_q4_word(
                        packed,
                        x01,
                        x23,
                        x45,
                        x67,
                        group_accum
                    );
                    x_index += 8;
                } else {
                    #pragma unroll
                    for (uint32_t elem = 0; elem < pack_factor; ++elem) {
                        if (x_index >= n_in) {
                            break;
                        }
                        const float x = __bfloat162float(*reinterpret_cast<const __nv_bfloat16 *>(
                            batch_input + x_index
                        ));
                        group_sum = __fadd_rn(group_sum, x);
                        group_accum = __fadd_rn(
                            group_accum,
                            __fmul_rn(x, static_cast<float>(packed & mask))
                        );
                        packed >>= BITS;
                        ++x_index;
                    }
                }
            }
        }

        thread_total = __fadd_rn(
            thread_total,
            __fadd_rn(
                __fmul_rn(scale, group_accum),
                __fmul_rn(bias, group_sum)
            )
        );
    }

    thread_total = makepad_ggml_cuda_block_reduce_sum(thread_total);

    if (tid == 0) {
        output_f32[batch * out_rows + row] = thread_total;
    }
}

template <int BITS, int MAX_SLOTS, int FIXED_SELECTED_COUNT = 0, bool KNOWN_VALID = false>
static __global__ void makepad_ggml_cuda_affine_qmv_f32_select_planes_precise_kernel(
    const uint16_t * __restrict__ input_bf16_words,
    const uint32_t * __restrict__ packed_weights_u32,
    const uint16_t * __restrict__ scales_bf16_words,
    const uint16_t * __restrict__ biases_bf16_words,
    const uint32_t * __restrict__ plane_indices_u32,
    const uint32_t selected_count,
    float * __restrict__ output_f32,
    const uint32_t n_in,
    const uint32_t weight_words_per_row,
    const uint32_t qparams_per_row,
    const uint32_t out_rows,
    const uint32_t weight_words_per_plane,
    const uint32_t qparams_words_per_plane,
    const uint32_t plane_count
) {
    const uint32_t row = blockIdx.x;
    if (row >= out_rows) {
        return;
    }

    constexpr uint32_t pack_factor = 32 / BITS;
    constexpr uint32_t group_size = 64;
    constexpr uint32_t words_per_group = group_size / pack_factor;
    constexpr uint32_t mask = (1u << BITS) - 1u;

    const uint32_t tid = threadIdx.x;
    uint32_t planes[MAX_SLOTS];
    bool slot_active[MAX_SLOTS];
    uint32_t weight_row_starts[MAX_SLOTS];
    uint32_t qparam_row_starts[MAX_SLOTS];
    float thread_total[MAX_SLOTS];

    #pragma unroll
    for (uint32_t slot = 0; slot < MAX_SLOTS; ++slot) {
        planes[slot] = 0;
        slot_active[slot] = false;
        weight_row_starts[slot] = 0;
        qparam_row_starts[slot] = 0;
        thread_total[slot] = 0.0f;
        constexpr bool has_fixed_selected = FIXED_SELECTED_COUNT > 0;
        if ((has_fixed_selected && slot < FIXED_SELECTED_COUNT)
            || (!has_fixed_selected && slot < selected_count)) {
            planes[slot] = plane_indices_u32[slot];
            if constexpr (KNOWN_VALID) {
                slot_active[slot] = true;
            } else {
                slot_active[slot] = planes[slot] < plane_count;
            }
            if (slot_active[slot]) {
                weight_row_starts[slot] =
                    planes[slot] * weight_words_per_plane + row * weight_words_per_row;
                qparam_row_starts[slot] =
                    planes[slot] * qparams_words_per_plane + row * qparams_per_row;
            }
        }
    }

    for (uint32_t group = tid; group < qparams_per_row; group += blockDim.x) {
        float group_sum = 0.0f;
        float group_accum[MAX_SLOTS];
        float scales[MAX_SLOTS];
        float biases[MAX_SLOTS];

        #pragma unroll
        for (uint32_t slot = 0; slot < MAX_SLOTS; ++slot) {
            group_accum[slot] = 0.0f;
            scales[slot] = 0.0f;
            biases[slot] = 0.0f;
            if (slot_active[slot]) {
                scales[slot] = __bfloat162float(*reinterpret_cast<const __nv_bfloat16 *>(
                    scales_bf16_words + qparam_row_starts[slot] + group
                ));
                biases[slot] = __bfloat162float(*reinterpret_cast<const __nv_bfloat16 *>(
                    biases_bf16_words + qparam_row_starts[slot] + group
                ));
            }
        }

        const uint32_t weight_group_offset = group * words_per_group;
        uint32_t x_index = group * group_size;

        #pragma unroll
        for (uint32_t word_offset = 0; word_offset < words_per_group; ++word_offset) {
            uint32_t packed[MAX_SLOTS];
            #pragma unroll
            for (uint32_t slot = 0; slot < MAX_SLOTS; ++slot) {
                packed[slot] = 0;
                if (slot_active[slot]) {
                    packed[slot] = packed_weights_u32[
                        weight_row_starts[slot] + weight_group_offset + word_offset
                    ];
                }
            }

            if constexpr (BITS == 4) {
                if (x_index + 7 < n_in) {
                    const float2 x01 = __bfloat1622float2(
                        *reinterpret_cast<const __nv_bfloat162 *>(input_bf16_words + x_index)
                    );
                    const float2 x23 = __bfloat1622float2(
                        *reinterpret_cast<const __nv_bfloat162 *>(input_bf16_words + x_index + 2)
                    );
                    const float2 x45 = __bfloat1622float2(
                        *reinterpret_cast<const __nv_bfloat162 *>(input_bf16_words + x_index + 4)
                    );
                    const float2 x67 = __bfloat1622float2(
                        *reinterpret_cast<const __nv_bfloat162 *>(input_bf16_words + x_index + 6)
                    );
                    group_sum = __fadd_rn(group_sum, x01.x);
                    group_sum = __fadd_rn(group_sum, x01.y);
                    group_sum = __fadd_rn(group_sum, x23.x);
                    group_sum = __fadd_rn(group_sum, x23.y);
                    group_sum = __fadd_rn(group_sum, x45.x);
                    group_sum = __fadd_rn(group_sum, x45.y);
                    group_sum = __fadd_rn(group_sum, x67.x);
                    group_sum = __fadd_rn(group_sum, x67.y);

                    #pragma unroll
                    for (uint32_t slot = 0; slot < MAX_SLOTS; ++slot) {
                        if (slot_active[slot]) {
                            makepad_ggml_cuda_affine_accum_q4_word(
                                packed[slot],
                                x01,
                                x23,
                                x45,
                                x67,
                                group_accum[slot]
                            );
                        }
                    }
                    x_index += 8;
                } else {
                    #pragma unroll
                    for (uint32_t elem = 0; elem < pack_factor; ++elem) {
                        if (x_index >= n_in) {
                            break;
                        }

                        const float x = __bfloat162float(*reinterpret_cast<const __nv_bfloat16 *>(
                            input_bf16_words + x_index
                        ));
                        group_sum = __fadd_rn(group_sum, x);

                        #pragma unroll
                        for (uint32_t slot = 0; slot < MAX_SLOTS; ++slot) {
                            if (slot_active[slot]) {
                                group_accum[slot] = __fadd_rn(
                                    group_accum[slot],
                                    __fmul_rn(x, static_cast<float>(packed[slot] & mask))
                                );
                                packed[slot] >>= BITS;
                            }
                        }

                        ++x_index;
                    }
                }
            } else {
                #pragma unroll
                for (uint32_t elem = 0; elem < pack_factor; ++elem) {
                    if (x_index >= n_in) {
                        break;
                    }

                    const float x = __bfloat162float(*reinterpret_cast<const __nv_bfloat16 *>(
                        input_bf16_words + x_index
                    ));
                    group_sum = __fadd_rn(group_sum, x);

                    #pragma unroll
                    for (uint32_t slot = 0; slot < MAX_SLOTS; ++slot) {
                        if (slot_active[slot]) {
                            group_accum[slot] = __fadd_rn(
                                group_accum[slot],
                                __fmul_rn(x, static_cast<float>(packed[slot] & mask))
                            );
                            packed[slot] >>= BITS;
                        }
                    }

                    ++x_index;
                }
            }
        }

        #pragma unroll
        for (uint32_t slot = 0; slot < MAX_SLOTS; ++slot) {
            if (slot_active[slot]) {
                thread_total[slot] = __fadd_rn(
                    thread_total[slot],
                    __fadd_rn(
                        __fmul_rn(scales[slot], group_accum[slot]),
                        __fmul_rn(biases[slot], group_sum)
                    )
                );
            }
        }
    }

    makepad_ggml_cuda_block_reduce_sum_slots(thread_total);

    if (tid == 0) {
        #pragma unroll
        for (uint32_t slot = 0; slot < MAX_SLOTS; ++slot) {
            constexpr bool has_fixed_selected = FIXED_SELECTED_COUNT > 0;
            if ((has_fixed_selected && slot < FIXED_SELECTED_COUNT)
                || (!has_fixed_selected && slot < selected_count)) {
                output_f32[slot * out_rows + row] = slot_active[slot] ? thread_total[slot] : 0.0f;
            }
        }
    }
}

template <int BITS, int MAX_SLOTS, int FIXED_SELECTED_COUNT = 0, bool KNOWN_VALID = false>
static __global__ void makepad_ggml_cuda_affine_qmv_f32_select_planes_input_offsets_precise_kernel(
    const uint16_t * __restrict__ input_bf16_words,
    const uint32_t input_words_per_slot,
    const uint32_t * __restrict__ packed_weights_u32,
    const uint16_t * __restrict__ scales_bf16_words,
    const uint16_t * __restrict__ biases_bf16_words,
    const uint32_t * __restrict__ plane_indices_u32,
    const uint32_t selected_count,
    float * __restrict__ output_f32,
    const uint32_t n_in,
    const uint32_t weight_words_per_row,
    const uint32_t qparams_per_row,
    const uint32_t out_rows,
    const uint32_t weight_words_per_plane,
    const uint32_t qparams_words_per_plane,
    const uint32_t plane_count
) {
    const uint32_t row = blockIdx.x;
    if (row >= out_rows) {
        return;
    }

    constexpr uint32_t pack_factor = 32 / BITS;
    constexpr uint32_t group_size = 64;
    constexpr uint32_t words_per_group = group_size / pack_factor;
    constexpr uint32_t mask = (1u << BITS) - 1u;

    const uint32_t tid = threadIdx.x;
    uint32_t planes[MAX_SLOTS];
    bool slot_active[MAX_SLOTS];
    uint32_t weight_row_starts[MAX_SLOTS];
    uint32_t qparam_row_starts[MAX_SLOTS];
    const uint16_t * input_slots[MAX_SLOTS];
    float thread_total[MAX_SLOTS];

    #pragma unroll
    for (uint32_t slot = 0; slot < MAX_SLOTS; ++slot) {
        planes[slot] = 0;
        slot_active[slot] = false;
        weight_row_starts[slot] = 0;
        qparam_row_starts[slot] = 0;
        input_slots[slot] = input_bf16_words;
        thread_total[slot] = 0.0f;
        constexpr bool has_fixed_selected = FIXED_SELECTED_COUNT > 0;
        if ((has_fixed_selected && slot < FIXED_SELECTED_COUNT)
            || (!has_fixed_selected && slot < selected_count)) {
            planes[slot] = plane_indices_u32[slot];
            input_slots[slot] = input_bf16_words + slot * input_words_per_slot;
            if constexpr (KNOWN_VALID) {
                slot_active[slot] = true;
            } else {
                slot_active[slot] = planes[slot] < plane_count;
            }
            if (slot_active[slot]) {
                weight_row_starts[slot] =
                    planes[slot] * weight_words_per_plane + row * weight_words_per_row;
                qparam_row_starts[slot] =
                    planes[slot] * qparams_words_per_plane + row * qparams_per_row;
            }
        }
    }

    for (uint32_t group = tid; group < qparams_per_row; group += blockDim.x) {
        float group_accum[MAX_SLOTS];
        float group_sum[MAX_SLOTS];
        float scales[MAX_SLOTS];
        float biases[MAX_SLOTS];

        #pragma unroll
        for (uint32_t slot = 0; slot < MAX_SLOTS; ++slot) {
            group_accum[slot] = 0.0f;
            group_sum[slot] = 0.0f;
            scales[slot] = 0.0f;
            biases[slot] = 0.0f;
            if (slot_active[slot]) {
                scales[slot] = __bfloat162float(*reinterpret_cast<const __nv_bfloat16 *>(
                    scales_bf16_words + qparam_row_starts[slot] + group
                ));
                biases[slot] = __bfloat162float(*reinterpret_cast<const __nv_bfloat16 *>(
                    biases_bf16_words + qparam_row_starts[slot] + group
                ));
            }
        }

        const uint32_t weight_group_offset = group * words_per_group;
        const uint32_t group_input_offset = group * group_size;

        #pragma unroll
        for (uint32_t word_offset = 0; word_offset < words_per_group; ++word_offset) {
            uint32_t packed[MAX_SLOTS];
            #pragma unroll
            for (uint32_t slot = 0; slot < MAX_SLOTS; ++slot) {
                packed[slot] = 0;
                if (slot_active[slot]) {
                    packed[slot] = packed_weights_u32[
                        weight_row_starts[slot] + weight_group_offset + word_offset
                    ];
                }
            }

            const uint32_t x_index = group_input_offset + word_offset * pack_factor;
            if constexpr (BITS == 4) {
                if (x_index + 7 < n_in) {
                    #pragma unroll
                    for (uint32_t slot = 0; slot < MAX_SLOTS; ++slot) {
                        if (slot_active[slot]) {
                            const float2 x01 = __bfloat1622float2(
                                *reinterpret_cast<const __nv_bfloat162 *>(input_slots[slot] + x_index)
                            );
                            const float2 x23 = __bfloat1622float2(
                                *reinterpret_cast<const __nv_bfloat162 *>(input_slots[slot] + x_index + 2)
                            );
                            const float2 x45 = __bfloat1622float2(
                                *reinterpret_cast<const __nv_bfloat162 *>(input_slots[slot] + x_index + 4)
                            );
                            const float2 x67 = __bfloat1622float2(
                                *reinterpret_cast<const __nv_bfloat162 *>(input_slots[slot] + x_index + 6)
                            );
                            group_sum[slot] = __fadd_rn(group_sum[slot], x01.x);
                            group_sum[slot] = __fadd_rn(group_sum[slot], x01.y);
                            group_sum[slot] = __fadd_rn(group_sum[slot], x23.x);
                            group_sum[slot] = __fadd_rn(group_sum[slot], x23.y);
                            group_sum[slot] = __fadd_rn(group_sum[slot], x45.x);
                            group_sum[slot] = __fadd_rn(group_sum[slot], x45.y);
                            group_sum[slot] = __fadd_rn(group_sum[slot], x67.x);
                            group_sum[slot] = __fadd_rn(group_sum[slot], x67.y);
                            makepad_ggml_cuda_affine_accum_q4_word(
                                packed[slot],
                                x01,
                                x23,
                                x45,
                                x67,
                                group_accum[slot]
                            );
                        }
                    }
                } else {
                    #pragma unroll
                    for (uint32_t elem = 0; elem < pack_factor; ++elem) {
                        const uint32_t tail_index = x_index + elem;
                        if (tail_index >= n_in) {
                            break;
                        }

                        #pragma unroll
                        for (uint32_t slot = 0; slot < MAX_SLOTS; ++slot) {
                            if (slot_active[slot]) {
                                const float x = __bfloat162float(*reinterpret_cast<const __nv_bfloat16 *>(
                                    input_slots[slot] + tail_index
                                ));
                                group_sum[slot] = __fadd_rn(group_sum[slot], x);
                                group_accum[slot] = __fadd_rn(
                                    group_accum[slot],
                                    __fmul_rn(x, static_cast<float>(packed[slot] & mask))
                                );
                                packed[slot] >>= BITS;
                            }
                        }
                    }
                }
            } else {
                #pragma unroll
                for (uint32_t elem = 0; elem < pack_factor; ++elem) {
                    const uint32_t tail_index = x_index + elem;
                    if (tail_index >= n_in) {
                        break;
                    }

                    #pragma unroll
                    for (uint32_t slot = 0; slot < MAX_SLOTS; ++slot) {
                        if (slot_active[slot]) {
                            const float x = __bfloat162float(*reinterpret_cast<const __nv_bfloat16 *>(
                                input_slots[slot] + tail_index
                            ));
                            group_sum[slot] = __fadd_rn(group_sum[slot], x);
                            group_accum[slot] = __fadd_rn(
                                group_accum[slot],
                                __fmul_rn(x, static_cast<float>(packed[slot] & mask))
                            );
                            packed[slot] >>= BITS;
                        }
                    }
                }
            }
        }

        #pragma unroll
        for (uint32_t slot = 0; slot < MAX_SLOTS; ++slot) {
            if (slot_active[slot]) {
                thread_total[slot] = __fadd_rn(
                    thread_total[slot],
                    __fadd_rn(
                        __fmul_rn(scales[slot], group_accum[slot]),
                        __fmul_rn(biases[slot], group_sum[slot])
                    )
                );
            }
        }
    }

    makepad_ggml_cuda_block_reduce_sum_slots(thread_total);

    if (tid == 0) {
        #pragma unroll
        for (uint32_t slot = 0; slot < MAX_SLOTS; ++slot) {
            constexpr bool has_fixed_selected = FIXED_SELECTED_COUNT > 0;
            if ((has_fixed_selected && slot < FIXED_SELECTED_COUNT)
                || (!has_fixed_selected && slot < selected_count)) {
                output_f32[slot * out_rows + row] = slot_active[slot] ? thread_total[slot] : 0.0f;
            }
        }
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
    dim3 block(makepad_ggml_cuda_affine_block_size(qparams_per_row), 1, 1);
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

extern "C" cudaError_t makepad_ggml_cuda_affine_qmv_f32(
    const uint16_t * input_bf16_words,
    const uint32_t * packed_weights_u32,
    const uint16_t * scales_bf16_words,
    const uint16_t * biases_bf16_words,
    float * output_f32,
    uint32_t n_in,
    uint32_t weight_words_per_row,
    uint32_t qparams_per_row,
    uint32_t out_rows,
    uint32_t bits,
    cudaStream_t stream
) {
    dim3 block(makepad_ggml_cuda_affine_block_size(qparams_per_row), 1, 1);
    dim3 grid(out_rows, 1, 1);

    switch (bits) {
        case 4:
            makepad_ggml_cuda_affine_qmv_f32_kernel<4><<<grid, block, 0, stream>>>(
                input_bf16_words,
                packed_weights_u32,
                scales_bf16_words,
                biases_bf16_words,
                output_f32,
                n_in,
                weight_words_per_row,
                qparams_per_row,
                out_rows
            );
            break;
        case 8:
            makepad_ggml_cuda_affine_qmv_f32_kernel<8><<<grid, block, 0, stream>>>(
                input_bf16_words,
                packed_weights_u32,
                scales_bf16_words,
                biases_bf16_words,
                output_f32,
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

extern "C" cudaError_t makepad_ggml_cuda_affine_qmv_f32_precise(
    const uint16_t * input_bf16_words,
    const uint32_t * packed_weights_u32,
    const uint16_t * scales_bf16_words,
    const uint16_t * biases_bf16_words,
    float * output_f32,
    uint32_t n_in,
    uint32_t weight_words_per_row,
    uint32_t qparams_per_row,
    uint32_t out_rows,
    uint32_t bits,
    cudaStream_t stream
) {
    dim3 block(makepad_ggml_cuda_affine_block_size(qparams_per_row), 1, 1);
    dim3 grid(out_rows, 1, 1);

    switch (bits) {
        case 4:
            makepad_ggml_cuda_affine_qmv_f32_precise_kernel<4><<<grid, block, 0, stream>>>(
                input_bf16_words,
                packed_weights_u32,
                scales_bf16_words,
                biases_bf16_words,
                output_f32,
                n_in,
                weight_words_per_row,
                qparams_per_row,
                out_rows
            );
            break;
        case 8:
            makepad_ggml_cuda_affine_qmv_f32_precise_kernel<8><<<grid, block, 0, stream>>>(
                input_bf16_words,
                packed_weights_u32,
                scales_bf16_words,
                biases_bf16_words,
                output_f32,
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

extern "C" cudaError_t makepad_ggml_cuda_affine_q8_1_qmv_f32_precise(
    const uint16_t * input_bf16_words,
    const uint8_t * input_q8_1_bytes,
    const uint32_t * packed_weights_u32,
    const uint16_t * scales_bf16_words,
    const uint16_t * biases_bf16_words,
    float * output_f32,
    uint32_t n_in,
    uint32_t weight_words_per_row,
    uint32_t qparams_per_row,
    uint32_t out_rows,
    uint32_t bits,
    cudaStream_t stream
) {
    if ((n_in % 64u) != 0) {
        return cudaErrorInvalidValue;
    }

    dim3 block(makepad_ggml_cuda_affine_block_size(qparams_per_row), 1, 1);
    dim3 grid(out_rows, 1, 1);

    switch (bits) {
        case 4:
            makepad_ggml_cuda_affine_q8_1_qmv_f32_precise_kernel<4><<<grid, block, 0, stream>>>(
                input_bf16_words,
                reinterpret_cast<const block_q8_1 *>(input_q8_1_bytes),
                packed_weights_u32,
                scales_bf16_words,
                biases_bf16_words,
                output_f32,
                weight_words_per_row,
                qparams_per_row,
                out_rows
            );
            break;
        case 8:
            makepad_ggml_cuda_affine_q8_1_qmv_f32_precise_kernel<8><<<grid, block, 0, stream>>>(
                input_bf16_words,
                reinterpret_cast<const block_q8_1 *>(input_q8_1_bytes),
                packed_weights_u32,
                scales_bf16_words,
                biases_bf16_words,
                output_f32,
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

extern "C" cudaError_t makepad_ggml_cuda_affine_qmv_f32_rows_precise(
    const uint16_t * input_bf16_words,
    const uint32_t * packed_weights_u32,
    const uint16_t * scales_bf16_words,
    const uint16_t * biases_bf16_words,
    float * output_f32,
    uint32_t n_in,
    uint32_t weight_words_per_row,
    uint32_t qparams_per_row,
    uint32_t out_rows,
    uint32_t input_rows,
    uint32_t bits,
    cudaStream_t stream
) {
    if (n_in == 0 || out_rows == 0 || input_rows == 0) {
        return cudaSuccess;
    }

    constexpr uint32_t row_tile = 8;
    dim3 block(makepad_ggml_cuda_affine_block_size(qparams_per_row), 1, 1);
    dim3 grid(out_rows, (input_rows + row_tile - 1) / row_tile, 1);

    switch (bits) {
        case 4:
            makepad_ggml_cuda_affine_qmv_f32_rows_precise_kernel<4, row_tile><<<grid, block, 0, stream>>>(
                input_bf16_words,
                packed_weights_u32,
                scales_bf16_words,
                biases_bf16_words,
                output_f32,
                n_in,
                weight_words_per_row,
                qparams_per_row,
                out_rows,
                input_rows
            );
            break;
        case 8:
            makepad_ggml_cuda_affine_qmv_f32_rows_precise_kernel<8, row_tile><<<grid, block, 0, stream>>>(
                input_bf16_words,
                packed_weights_u32,
                scales_bf16_words,
                biases_bf16_words,
                output_f32,
                n_in,
                weight_words_per_row,
                qparams_per_row,
                out_rows,
                input_rows
            );
            break;
        default:
            return cudaErrorInvalidValue;
    }
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_affine_qmv_f32_select_plane_precise(
    const uint16_t * input_bf16_words,
    const uint32_t * packed_weights_u32,
    const uint16_t * scales_bf16_words,
    const uint16_t * biases_bf16_words,
    const uint32_t * plane_indices_u32,
    uint32_t plane_slot,
    float * output_f32,
    uint32_t n_in,
    uint32_t weight_words_per_row,
    uint32_t qparams_per_row,
    uint32_t out_rows,
    uint32_t weight_words_per_plane,
    uint32_t qparams_words_per_plane,
    uint32_t plane_count,
    uint32_t bits,
    cudaStream_t stream
) {
    if (out_rows == 0) {
        return cudaSuccess;
    }

    dim3 block(makepad_ggml_cuda_affine_block_size(qparams_per_row), 1, 1);
    dim3 grid(out_rows, 1, 1);

    switch (bits) {
        case 4:
            makepad_ggml_cuda_affine_qmv_f32_select_plane_precise_kernel<4><<<grid, block, 0, stream>>>(
                input_bf16_words,
                packed_weights_u32,
                scales_bf16_words,
                biases_bf16_words,
                plane_indices_u32,
                plane_slot,
                output_f32,
                n_in,
                weight_words_per_row,
                qparams_per_row,
                out_rows,
                weight_words_per_plane,
                qparams_words_per_plane,
                plane_count
            );
            break;
        case 8:
            makepad_ggml_cuda_affine_qmv_f32_select_plane_precise_kernel<8><<<grid, block, 0, stream>>>(
                input_bf16_words,
                packed_weights_u32,
                scales_bf16_words,
                biases_bf16_words,
                plane_indices_u32,
                plane_slot,
                output_f32,
                n_in,
                weight_words_per_row,
                qparams_per_row,
                out_rows,
                weight_words_per_plane,
                qparams_words_per_plane,
                plane_count
            );
            break;
        default:
            return cudaErrorInvalidValue;
    }

    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_affine_qmv_f32_select_plane_rows_precise(
    const uint16_t * input_bf16_words,
    const uint32_t * packed_weights_u32,
    const uint16_t * scales_bf16_words,
    const uint16_t * biases_bf16_words,
    const uint32_t * plane_indices_u32,
    uint32_t plane_indices_row_stride,
    uint32_t plane_slot,
    float * output_f32,
    uint32_t n_in,
    uint32_t weight_words_per_row,
    uint32_t qparams_per_row,
    uint32_t out_rows,
    uint32_t input_rows,
    uint32_t weight_words_per_plane,
    uint32_t qparams_words_per_plane,
    uint32_t plane_count,
    uint32_t bits,
    cudaStream_t stream
) {
    if (out_rows == 0 || input_rows == 0) {
        return cudaSuccess;
    }

    dim3 block(makepad_ggml_cuda_affine_block_size(qparams_per_row), 1, 1);
    dim3 grid(out_rows, input_rows, 1);

    switch (bits) {
        case 4:
            makepad_ggml_cuda_affine_qmv_f32_select_plane_rows_precise_kernel<4><<<grid, block, 0, stream>>>(
                input_bf16_words,
                packed_weights_u32,
                scales_bf16_words,
                biases_bf16_words,
                plane_indices_u32,
                plane_indices_row_stride,
                plane_slot,
                output_f32,
                n_in,
                weight_words_per_row,
                qparams_per_row,
                out_rows,
                input_rows,
                weight_words_per_plane,
                qparams_words_per_plane,
                plane_count
            );
            break;
        case 8:
            makepad_ggml_cuda_affine_qmv_f32_select_plane_rows_precise_kernel<8><<<grid, block, 0, stream>>>(
                input_bf16_words,
                packed_weights_u32,
                scales_bf16_words,
                biases_bf16_words,
                plane_indices_u32,
                plane_indices_row_stride,
                plane_slot,
                output_f32,
                n_in,
                weight_words_per_row,
                qparams_per_row,
                out_rows,
                input_rows,
                weight_words_per_plane,
                qparams_words_per_plane,
                plane_count
            );
            break;
        default:
            return cudaErrorInvalidValue;
    }

    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_affine_qmv_f32_select_planes_precise(
    const uint16_t * input_bf16_words,
    const uint32_t * packed_weights_u32,
    const uint16_t * scales_bf16_words,
    const uint16_t * biases_bf16_words,
    const uint32_t * plane_indices_u32,
    uint32_t selected_count,
    float * output_f32,
    uint32_t n_in,
    uint32_t weight_words_per_row,
    uint32_t qparams_per_row,
    uint32_t out_rows,
    uint32_t weight_words_per_plane,
    uint32_t qparams_words_per_plane,
    uint32_t plane_count,
    uint32_t bits,
    cudaStream_t stream
) {
    if (out_rows == 0 || selected_count == 0) {
        return cudaSuccess;
    }
    if (selected_count > 8) {
        return cudaErrorInvalidValue;
    }

    dim3 block(makepad_ggml_cuda_affine_block_size(qparams_per_row), 1, 1);
    dim3 grid(out_rows, 1, 1);

    switch (bits) {
        case 4:
            if (selected_count <= 4) {
                makepad_ggml_cuda_affine_qmv_f32_select_planes_precise_kernel<4, 4, 0, false><<<grid, block, 0, stream>>>(
                    input_bf16_words,
                    packed_weights_u32,
                    scales_bf16_words,
                    biases_bf16_words,
                    plane_indices_u32,
                    selected_count,
                    output_f32,
                    n_in,
                    weight_words_per_row,
                    qparams_per_row,
                    out_rows,
                    weight_words_per_plane,
                    qparams_words_per_plane,
                    plane_count
                );
            } else {
                makepad_ggml_cuda_affine_qmv_f32_select_planes_precise_kernel<4, 8, 0, false><<<grid, block, 0, stream>>>(
                    input_bf16_words,
                    packed_weights_u32,
                    scales_bf16_words,
                    biases_bf16_words,
                    plane_indices_u32,
                    selected_count,
                    output_f32,
                    n_in,
                    weight_words_per_row,
                    qparams_per_row,
                    out_rows,
                    weight_words_per_plane,
                    qparams_words_per_plane,
                    plane_count
                );
            }
            break;
        case 8:
            if (selected_count <= 4) {
                makepad_ggml_cuda_affine_qmv_f32_select_planes_precise_kernel<8, 4, 0, false><<<grid, block, 0, stream>>>(
                    input_bf16_words,
                    packed_weights_u32,
                    scales_bf16_words,
                    biases_bf16_words,
                    plane_indices_u32,
                    selected_count,
                    output_f32,
                    n_in,
                    weight_words_per_row,
                    qparams_per_row,
                    out_rows,
                    weight_words_per_plane,
                    qparams_words_per_plane,
                    plane_count
                );
            } else {
                makepad_ggml_cuda_affine_qmv_f32_select_planes_precise_kernel<8, 8, 0, false><<<grid, block, 0, stream>>>(
                    input_bf16_words,
                    packed_weights_u32,
                    scales_bf16_words,
                    biases_bf16_words,
                    plane_indices_u32,
                    selected_count,
                    output_f32,
                    n_in,
                    weight_words_per_row,
                    qparams_per_row,
                    out_rows,
                    weight_words_per_plane,
                    qparams_words_per_plane,
                    plane_count
                );
            }
            break;
        default:
            return cudaErrorInvalidValue;
    }

    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_affine_qmv_f32_select_planes_fixed8_known_valid_precise(
    const uint16_t * input_bf16_words,
    const uint32_t * packed_weights_u32,
    const uint16_t * scales_bf16_words,
    const uint16_t * biases_bf16_words,
    const uint32_t * plane_indices_u32,
    float * output_f32,
    uint32_t n_in,
    uint32_t weight_words_per_row,
    uint32_t qparams_per_row,
    uint32_t out_rows,
    uint32_t weight_words_per_plane,
    uint32_t qparams_words_per_plane,
    uint32_t bits,
    cudaStream_t stream
) {
    if (out_rows == 0) {
        return cudaSuccess;
    }

    dim3 block(makepad_ggml_cuda_affine_block_size(qparams_per_row), 1, 1);
    dim3 grid(out_rows, 1, 1);

    switch (bits) {
        case 4:
            makepad_ggml_cuda_affine_qmv_f32_select_planes_precise_kernel<4, 8, 8, true><<<grid, block, 0, stream>>>(
                input_bf16_words,
                packed_weights_u32,
                scales_bf16_words,
                biases_bf16_words,
                plane_indices_u32,
                8,
                output_f32,
                n_in,
                weight_words_per_row,
                qparams_per_row,
                out_rows,
                weight_words_per_plane,
                qparams_words_per_plane,
                0
            );
            break;
        case 8:
            makepad_ggml_cuda_affine_qmv_f32_select_planes_precise_kernel<8, 8, 8, true><<<grid, block, 0, stream>>>(
                input_bf16_words,
                packed_weights_u32,
                scales_bf16_words,
                biases_bf16_words,
                plane_indices_u32,
                8,
                output_f32,
                n_in,
                weight_words_per_row,
                qparams_per_row,
                out_rows,
                weight_words_per_plane,
                qparams_words_per_plane,
                0
            );
            break;
        default:
            return cudaErrorInvalidValue;
    }

    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_affine_qmv_f32_select_planes_input_offsets_precise(
    const uint16_t * input_bf16_words,
    uint32_t input_words_per_slot,
    const uint32_t * packed_weights_u32,
    const uint16_t * scales_bf16_words,
    const uint16_t * biases_bf16_words,
    const uint32_t * plane_indices_u32,
    uint32_t selected_count,
    float * output_f32,
    uint32_t n_in,
    uint32_t weight_words_per_row,
    uint32_t qparams_per_row,
    uint32_t out_rows,
    uint32_t weight_words_per_plane,
    uint32_t qparams_words_per_plane,
    uint32_t plane_count,
    uint32_t bits,
    cudaStream_t stream
) {
    if (out_rows == 0 || selected_count == 0) {
        return cudaSuccess;
    }
    if (selected_count > 8) {
        return cudaErrorInvalidValue;
    }

    dim3 block(makepad_ggml_cuda_affine_block_size(qparams_per_row), 1, 1);
    dim3 grid(out_rows, 1, 1);

    switch (bits) {
        case 4:
            if (selected_count <= 4) {
                makepad_ggml_cuda_affine_qmv_f32_select_planes_input_offsets_precise_kernel<4, 4, 0, false><<<grid, block, 0, stream>>>(
                    input_bf16_words,
                    input_words_per_slot,
                    packed_weights_u32,
                    scales_bf16_words,
                    biases_bf16_words,
                    plane_indices_u32,
                    selected_count,
                    output_f32,
                    n_in,
                    weight_words_per_row,
                    qparams_per_row,
                    out_rows,
                    weight_words_per_plane,
                    qparams_words_per_plane,
                    plane_count
                );
            } else {
                makepad_ggml_cuda_affine_qmv_f32_select_planes_input_offsets_precise_kernel<4, 8, 0, false><<<grid, block, 0, stream>>>(
                    input_bf16_words,
                    input_words_per_slot,
                    packed_weights_u32,
                    scales_bf16_words,
                    biases_bf16_words,
                    plane_indices_u32,
                    selected_count,
                    output_f32,
                    n_in,
                    weight_words_per_row,
                    qparams_per_row,
                    out_rows,
                    weight_words_per_plane,
                    qparams_words_per_plane,
                    plane_count
                );
            }
            break;
        case 8:
            if (selected_count <= 4) {
                makepad_ggml_cuda_affine_qmv_f32_select_planes_input_offsets_precise_kernel<8, 4, 0, false><<<grid, block, 0, stream>>>(
                    input_bf16_words,
                    input_words_per_slot,
                    packed_weights_u32,
                    scales_bf16_words,
                    biases_bf16_words,
                    plane_indices_u32,
                    selected_count,
                    output_f32,
                    n_in,
                    weight_words_per_row,
                    qparams_per_row,
                    out_rows,
                    weight_words_per_plane,
                    qparams_words_per_plane,
                    plane_count
                );
            } else {
                makepad_ggml_cuda_affine_qmv_f32_select_planes_input_offsets_precise_kernel<8, 8, 0, false><<<grid, block, 0, stream>>>(
                    input_bf16_words,
                    input_words_per_slot,
                    packed_weights_u32,
                    scales_bf16_words,
                    biases_bf16_words,
                    plane_indices_u32,
                    selected_count,
                    output_f32,
                    n_in,
                    weight_words_per_row,
                    qparams_per_row,
                    out_rows,
                    weight_words_per_plane,
                    qparams_words_per_plane,
                    plane_count
                );
            }
            break;
        default:
            return cudaErrorInvalidValue;
    }

    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_affine_qmv_f32_select_planes_input_offsets_fixed8_known_valid_precise(
    const uint16_t * input_bf16_words,
    uint32_t input_words_per_slot,
    const uint32_t * packed_weights_u32,
    const uint16_t * scales_bf16_words,
    const uint16_t * biases_bf16_words,
    const uint32_t * plane_indices_u32,
    float * output_f32,
    uint32_t n_in,
    uint32_t weight_words_per_row,
    uint32_t qparams_per_row,
    uint32_t out_rows,
    uint32_t weight_words_per_plane,
    uint32_t qparams_words_per_plane,
    uint32_t bits,
    cudaStream_t stream
) {
    if (out_rows == 0) {
        return cudaSuccess;
    }

    dim3 block(makepad_ggml_cuda_affine_block_size(qparams_per_row), 1, 1);
    dim3 grid(out_rows, 1, 1);

    switch (bits) {
        case 4:
            makepad_ggml_cuda_affine_qmv_f32_select_planes_input_offsets_precise_kernel<4, 8, 8, true><<<grid, block, 0, stream>>>(
                input_bf16_words,
                input_words_per_slot,
                packed_weights_u32,
                scales_bf16_words,
                biases_bf16_words,
                plane_indices_u32,
                8,
                output_f32,
                n_in,
                weight_words_per_row,
                qparams_per_row,
                out_rows,
                weight_words_per_plane,
                qparams_words_per_plane,
                0
            );
            break;
        case 8:
            makepad_ggml_cuda_affine_qmv_f32_select_planes_input_offsets_precise_kernel<8, 8, 8, true><<<grid, block, 0, stream>>>(
                input_bf16_words,
                input_words_per_slot,
                packed_weights_u32,
                scales_bf16_words,
                biases_bf16_words,
                plane_indices_u32,
                8,
                output_f32,
                n_in,
                weight_words_per_row,
                qparams_per_row,
                out_rows,
                weight_words_per_plane,
                qparams_words_per_plane,
                0
            );
            break;
        default:
            return cudaErrorInvalidValue;
    }

    return cudaGetLastError();
}

template <int BITS>
static __global__ void makepad_ggml_cuda_affine_get_row_f32_kernel(
    const uint32_t * packed_weights_u32,
    const uint16_t * scales_bf16_words,
    const uint16_t * biases_bf16_words,
    float * output_f32,
    const uint32_t weight_words_per_row,
    const uint32_t qparams_per_row,
    const uint32_t row_index
) {
    constexpr uint32_t pack_factor = 32 / BITS;
    constexpr uint32_t group_size = 64;
    constexpr uint32_t mask = (1u << BITS) - 1u;

    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    const uint32_t out_cols = weight_words_per_row * pack_factor;
    if (idx >= out_cols) {
        return;
    }

    const uint32_t qparam_row_start = row_index * qparams_per_row;
    const uint32_t weight_row_start = row_index * weight_words_per_row;
    const uint32_t group = idx / group_size;
    const uint32_t offset_in_group = idx % group_size;
    const uint32_t packed_idx = weight_row_start + group * (group_size / pack_factor) + offset_in_group / pack_factor;
    const uint32_t shift = (offset_in_group % pack_factor) * BITS;
    const float scale = __bfloat162float(*reinterpret_cast<const __nv_bfloat16 *>(
        scales_bf16_words + qparam_row_start + group
    ));
    const float bias = __bfloat162float(*reinterpret_cast<const __nv_bfloat16 *>(
        biases_bf16_words + qparam_row_start + group
    ));
    const uint32_t packed = packed_weights_u32[packed_idx];
    const float q = static_cast<float>((packed >> shift) & mask);
    output_f32[idx] = bf16_round_f32(__fadd_rn(__fmul_rn(scale, q), bias));
}

template <int BITS>
static __global__ void makepad_ggml_cuda_affine_get_row_f32_device_u32_kernel(
    const uint32_t * packed_weights_u32,
    const uint16_t * scales_bf16_words,
    const uint16_t * biases_bf16_words,
    float * output_f32,
    const uint32_t weight_words_per_row,
    const uint32_t qparams_per_row,
    const uint32_t * row_index_device_u32
) {
    const uint32_t row_index = *row_index_device_u32;
    constexpr uint32_t pack_factor = 32 / BITS;
    constexpr uint32_t group_size = 64;
    constexpr uint32_t mask = (1u << BITS) - 1u;

    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    const uint32_t out_cols = weight_words_per_row * pack_factor;
    if (idx >= out_cols) {
        return;
    }

    const uint32_t qparam_row_start = row_index * qparams_per_row;
    const uint32_t weight_row_start = row_index * weight_words_per_row;
    const uint32_t group = idx / group_size;
    const uint32_t offset_in_group = idx % group_size;
    const uint32_t packed_idx = weight_row_start + group * (group_size / pack_factor) + offset_in_group / pack_factor;
    const uint32_t shift = (offset_in_group % pack_factor) * BITS;
    const float scale = __bfloat162float(*reinterpret_cast<const __nv_bfloat16 *>(
        scales_bf16_words + qparam_row_start + group
    ));
    const float bias = __bfloat162float(*reinterpret_cast<const __nv_bfloat16 *>(
        biases_bf16_words + qparam_row_start + group
    ));
    const uint32_t packed = packed_weights_u32[packed_idx];
    const float q = static_cast<float>((packed >> shift) & mask);
    output_f32[idx] = bf16_round_f32(__fadd_rn(__fmul_rn(scale, q), bias));
}

extern "C" cudaError_t makepad_ggml_cuda_affine_get_row_f32(
    const uint32_t * packed_weights_u32,
    const uint16_t * scales_bf16_words,
    const uint16_t * biases_bf16_words,
    float * output_f32,
    uint32_t weight_words_per_row,
    uint32_t qparams_per_row,
    uint32_t row_index,
    uint32_t bits,
    cudaStream_t stream
) {
    const uint32_t pack_factor = 32 / bits;
    if (pack_factor == 0) {
        return cudaErrorInvalidValue;
    }
    const uint32_t out_cols = weight_words_per_row * pack_factor;
    const dim3 block(256, 1, 1);
    const dim3 grid((out_cols + block.x - 1) / block.x, 1, 1);

    switch (bits) {
        case 4:
            makepad_ggml_cuda_affine_get_row_f32_kernel<4><<<grid, block, 0, stream>>>(
                packed_weights_u32,
                scales_bf16_words,
                biases_bf16_words,
                output_f32,
                weight_words_per_row,
                qparams_per_row,
                row_index
            );
            break;
        case 8:
            makepad_ggml_cuda_affine_get_row_f32_kernel<8><<<grid, block, 0, stream>>>(
                packed_weights_u32,
                scales_bf16_words,
                biases_bf16_words,
                output_f32,
                weight_words_per_row,
                qparams_per_row,
                row_index
            );
            break;
        default:
            return cudaErrorInvalidValue;
    }

    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_affine_get_row_f32_device_u32(
    const uint32_t * packed_weights_u32,
    const uint16_t * scales_bf16_words,
    const uint16_t * biases_bf16_words,
    float * output_f32,
    uint32_t weight_words_per_row,
    uint32_t qparams_per_row,
    const uint32_t * row_index_device_u32,
    uint32_t bits,
    cudaStream_t stream
) {
    const uint32_t pack_factor = 32 / bits;
    if (pack_factor == 0) {
        return cudaErrorInvalidValue;
    }
    const uint32_t out_cols = weight_words_per_row * pack_factor;
    const dim3 block(256, 1, 1);
    const dim3 grid((out_cols + block.x - 1) / block.x, 1, 1);

    switch (bits) {
        case 4:
            makepad_ggml_cuda_affine_get_row_f32_device_u32_kernel<4><<<grid, block, 0, stream>>>(
                packed_weights_u32,
                scales_bf16_words,
                biases_bf16_words,
                output_f32,
                weight_words_per_row,
                qparams_per_row,
                row_index_device_u32
            );
            break;
        case 8:
            makepad_ggml_cuda_affine_get_row_f32_device_u32_kernel<8><<<grid, block, 0, stream>>>(
                packed_weights_u32,
                scales_bf16_words,
                biases_bf16_words,
                output_f32,
                weight_words_per_row,
                qparams_per_row,
                row_index_device_u32
            );
            break;
        default:
            return cudaErrorInvalidValue;
    }

    return cudaGetLastError();
}
