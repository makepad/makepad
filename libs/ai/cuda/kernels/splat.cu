// TripoSplat device ops.
//
// The flow denoiser's positional encoding (`RePo3DRotaryEmbedding` in the
// released model.py) is a LEARNED, per-token AND per-head rope: each block
// projects its own hidden state down to three deltas per head and turns them
// into head_dim/2 complex phases. That is two things the shared rope kernels
// cannot do — the existing `rope_interleaved` table is one row per token,
// shared across heads, and it is supplied by the caller rather than derived
// from an activation. Round-tripping the tables through the host would move
// ~50 MB per block per forward (24 blocks x 20 steps x 2 CFG passes), so the
// tables are built and consumed entirely on device here.

#include <cuda_runtime.h>
#include <cstdint>

#ifndef M_PIf
#define M_PIf 3.14159265358979323846f
#endif

// Phase tables from one RePo3D delta projection.
//
//   delta : [tokens][heads][3]           (final_map output, reshaped)
//   freqs : [pairs]                      ([freqs_0 | freqs_1 | freqs_2])
//   cos/sin: [tokens][heads][pairs]
//
// `ang = delta[axis(p)] * freqs[p] * pi`, where axis(p) is 0 for the first
// `dim0` pairs, 1 for the next `dim1`, and 2 for the rest. The reference's
// `clamp_mul(x, f) = x*tanh(f) + x.detach()*(f - tanh(f))` is a straight
// `x * f` at inference (detach is the identity without autograd), which is
// what this computes.
static __global__ void makepad_cuda_splat_repo3d_tables_f32_kernel(
        const float * __restrict__ delta,
        const float * __restrict__ freqs,
        float * __restrict__ cos_out,
        float * __restrict__ sin_out,
        uint32_t token_count,
        uint32_t head_count,
        uint32_t pairs,
        uint32_t dim0,
        uint32_t dim1) {
    const uint32_t token = blockIdx.x;
    const uint32_t head = blockIdx.y;
    if (token >= token_count || head >= head_count) {
        return;
    }
    const size_t delta_base = (static_cast<size_t>(token) * head_count + head) * 3;
    const float d0 = delta[delta_base + 0];
    const float d1 = delta[delta_base + 1];
    const float d2 = delta[delta_base + 2];
    const size_t out_base = (static_cast<size_t>(token) * head_count + head) * pairs;
    for (uint32_t p = threadIdx.x; p < pairs; p += blockDim.x) {
        const float value = (p < dim0) ? d0 : ((p < dim0 + dim1) ? d1 : d2);
        const float angle = value * freqs[p] * M_PIf;
        float s;
        float c;
        sincosf(angle, &s, &c);
        cos_out[out_base + p] = c;
        sin_out[out_base + p] = s;
    }
}

extern "C" cudaError_t makepad_cuda_splat_repo3d_tables_f32(
        const float * delta,
        const float * freqs,
        float * cos_out,
        float * sin_out,
        uint32_t token_count,
        uint32_t head_count,
        uint32_t pairs,
        uint32_t dim0,
        uint32_t dim1,
        cudaStream_t stream) {
    if (token_count == 0 || head_count == 0 || pairs == 0) {
        return cudaSuccess;
    }
    const dim3 block(pairs < 64 ? 32 : 64, 1, 1);
    const dim3 grid(token_count, head_count, 1);
    makepad_cuda_splat_repo3d_tables_f32_kernel<<<grid, block, 0, stream>>>(
        delta, freqs, cos_out, sin_out, token_count, head_count, pairs, dim0, dim1);
    return cudaGetLastError();
}

// Interleaved-pair rope with a PER-HEAD phase table.
//
//   input  : [tokens][heads][2 * pairs]
//   cos/sin: [tokens][heads][pairs]
//
// Pairs are (2p, 2p+1) inside a head, matching torch.view_as_complex — the
// same pairing as the shared `rope_interleaved` kernel, but indexing a table
// that varies per head.
static __global__ void makepad_cuda_splat_rope_pairs_per_head_f32_kernel(
        const float * __restrict__ input,
        const float * __restrict__ cos_table,
        const float * __restrict__ sin_table,
        float * __restrict__ output,
        uint32_t token_count,
        uint32_t head_count,
        uint32_t pairs) {
    const uint32_t token = blockIdx.x;
    const uint32_t head = blockIdx.y;
    if (token >= token_count || head >= head_count) {
        return;
    }
    const size_t base = (static_cast<size_t>(token) * head_count + head)
        * (static_cast<size_t>(pairs) * 2);
    const size_t table_base = (static_cast<size_t>(token) * head_count + head) * pairs;
    for (uint32_t p = threadIdx.x; p < pairs; p += blockDim.x) {
        const size_t even = base + static_cast<size_t>(p) * 2;
        const float c = cos_table[table_base + p];
        const float s = sin_table[table_base + p];
        const float re = input[even];
        const float im = input[even + 1];
        output[even] = re * c - im * s;
        output[even + 1] = re * s + im * c;
    }
}

extern "C" cudaError_t makepad_cuda_splat_rope_pairs_per_head_f32(
        const float * input,
        const float * cos_table,
        const float * sin_table,
        float * output,
        uint32_t token_count,
        uint32_t head_count,
        uint32_t pairs,
        cudaStream_t stream) {
    if (token_count == 0 || head_count == 0 || pairs == 0) {
        return cudaSuccess;
    }
    const dim3 block(pairs < 64 ? 32 : 64, 1, 1);
    const dim3 grid(token_count, head_count, 1);
    makepad_cuda_splat_rope_pairs_per_head_f32_kernel<<<grid, block, 0, stream>>>(
        input, cos_table, sin_table, output, token_count, head_count, pairs);
    return cudaGetLastError();
}
