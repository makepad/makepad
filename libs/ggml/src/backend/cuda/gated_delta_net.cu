#include <cuda_runtime.h>
#include <math.h>
#include <stdint.h>

static __global__ void makepad_ggml_cuda_gated_delta_net_f32_kernel(
        const float * __restrict__ q,
        const float * __restrict__ k,
        const float * __restrict__ v,
        const float * __restrict__ g,
        const float * __restrict__ beta,
        const float * __restrict__ state,
        float * __restrict__ dst,
        uint32_t sv,
        uint32_t h,
        uint32_t n_tokens,
        uint32_t n_seqs,
        uint32_t sq1,
        uint32_t sq2,
        uint32_t sq3,
        uint32_t sv1,
        uint32_t sv2,
        uint32_t sv3,
        uint32_t sb1,
        uint32_t sb2,
        uint32_t sb3,
        uint32_t neqk1,
        uint32_t rq3,
        uint32_t kda) {
    const uint32_t head_idx = blockIdx.x;
    const uint32_t seq_idx = blockIdx.y;
    const uint32_t col = blockIdx.z;
    const uint32_t lane = threadIdx.x;
    if (col >= sv) {
        return;
    }

    extern __shared__ float smem[];
    float * state_shared = smem;
    float * reduce_shared = smem + sv;

    const uint32_t iq1 = head_idx % neqk1;
    const uint32_t iq3 = seq_idx / rq3;

    const uint32_t attn_elems = sv * h * n_tokens * n_seqs;
    float * attn_out = dst;
    float * state_out = dst + attn_elems;

    const uint32_t state_offset = (seq_idx * h + head_idx) * sv * sv;
    const float * state_col = state + state_offset + col * sv;
    float * state_col_out = state_out + state_offset + col * sv;
    const float scale = rsqrtf((float) sv);

    for (uint32_t row = lane; row < sv; row += blockDim.x) {
        state_shared[row] = state_col[row];
    }
    __syncthreads();

    for (uint32_t token_idx = 0; token_idx < n_tokens; token_idx++) {
        const float * q_t = q + iq3 * sq3 + token_idx * sq2 + iq1 * sq1;
        const float * k_t = k + iq3 * sq3 + token_idx * sq2 + iq1 * sq1;
        const float * v_t = v + seq_idx * sv3 + token_idx * sv2 + head_idx * sv1;
        const uint32_t gb_offset = seq_idx * sb3 + token_idx * sb2 + head_idx * sb1;
        const float * beta_t = beta + gb_offset;
        const float * g_t = g + (kda ? gb_offset * sv : gb_offset);

        float kv_partial = 0.0f;
        if (kda) {
            for (uint32_t row = lane; row < sv; row += blockDim.x) {
                kv_partial += expf(g_t[row]) * state_shared[row] * k_t[row];
            }
        } else {
            for (uint32_t row = lane; row < sv; row += blockDim.x) {
                kv_partial += state_shared[row] * k_t[row];
            }
        }
        reduce_shared[lane] = kv_partial;
        __syncthreads();
        for (uint32_t stride = blockDim.x >> 1; stride > 0; stride >>= 1) {
            if (lane < stride) {
                reduce_shared[lane] += reduce_shared[lane + stride];
            }
            __syncthreads();
        }

        const float beta_val = *beta_t;
        const float g_scalar = kda ? 0.0f : expf(*g_t);
        const float delta = kda
            ? (v_t[col] - reduce_shared[0]) * beta_val
            : (v_t[col] - g_scalar * reduce_shared[0]) * beta_val;

        float attn_partial = 0.0f;
        for (uint32_t row = lane; row < sv; row += blockDim.x) {
            const float gate = kda ? expf(g_t[row]) : g_scalar;
            const float updated = gate * state_shared[row] + k_t[row] * delta;
            state_shared[row] = updated;
            attn_partial += updated * q_t[row];
        }
        reduce_shared[lane] = attn_partial;
        __syncthreads();
        for (uint32_t stride = blockDim.x >> 1; stride > 0; stride >>= 1) {
            if (lane < stride) {
                reduce_shared[lane] += reduce_shared[lane + stride];
            }
            __syncthreads();
        }

        if (lane == 0) {
            attn_out[((seq_idx * n_tokens + token_idx) * h + head_idx) * sv + col] =
                reduce_shared[0] * scale;
        }
        __syncthreads();
    }

    for (uint32_t row = lane; row < sv; row += blockDim.x) {
        state_col_out[row] = state_shared[row];
    }
}

extern "C" cudaError_t makepad_ggml_cuda_gated_delta_net_f32(
        const float * q,
        const float * k,
        const float * v,
        const float * g,
        const float * beta,
        const float * state,
        float * dst,
        uint32_t sv,
        uint32_t h,
        uint32_t n_tokens,
        uint32_t n_seqs,
        uint32_t sq1,
        uint32_t sq2,
        uint32_t sq3,
        uint32_t sv1,
        uint32_t sv2,
        uint32_t sv3,
        uint32_t sb1,
        uint32_t sb2,
        uint32_t sb3,
        uint32_t neqk1,
        uint32_t rq3,
        uint32_t kda,
        cudaStream_t stream) {
    if (sv == 0 || h == 0 || n_seqs == 0) {
        return cudaSuccess;
    }
    const uint32_t block = sv <= 32 ? 32 : (sv <= 64 ? 64 : 128);
    const dim3 grid(h, n_seqs, sv);
    const size_t shared_bytes = (sv + block) * sizeof(float);
    makepad_ggml_cuda_gated_delta_net_f32_kernel<<<grid, block, shared_bytes, stream>>>(
        q,
        k,
        v,
        g,
        beta,
        state,
        dst,
        sv,
        h,
        n_tokens,
        n_seqs,
        sq1,
        sq2,
        sq3,
        sv1,
        sv2,
        sv3,
        sb1,
        sb2,
        sb3,
        neqk1,
        rq3,
        kda);
    return cudaGetLastError();
}
