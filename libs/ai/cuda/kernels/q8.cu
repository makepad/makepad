// GGML Q8_0 device weight quantization and decode GEMV.
// No expanded weight matrix or quantized activations.
#include <cuda_fp16.h>
#include <cuda_runtime.h>
#include <math.h>
#include <stdint.h>
#include "launch_profile.cuh"

struct block_q8_0 {
    half d;
    int8_t qs[32];
};
static_assert(sizeof(block_q8_0) == 34, "wrong Q8_0 block size");

// One warp quantizes one contiguous block of 32 BF16 weights. Preserve the
// former host quantizer's f32 division, reciprocal and multiplication as
// separate correctly rounded operations; x / scale or 127 / absmax can
// change a quant at a rounding boundary. Only the stored scale is f16.
static __global__ void q8_0_quantize_bf16_kernel(
        const uint16_t *__restrict__ src, block_q8_0 *__restrict__ dst,
        uint32_t blocks, uint32_t *__restrict__ nonfinite) {
    const uint32_t lane = threadIdx.x & 31;
    const uint32_t block = blockIdx.x * 4 + (threadIdx.x >> 5);
    if (block >= blocks) return; // Whole warp exits before any shuffle.

    const float value = __uint_as_float((uint32_t)src[(size_t)block * 32 + lane] << 16);
    if (__any_sync(0xffffffffu, !isfinite(value))) {
        if (lane == 0) atomicExch(nonfinite, 1u);
        return;
    }
    float absmax = fabsf(value);
    #pragma unroll
    for (int offset = 16; offset > 0; offset >>= 1) {
        absmax = fmaxf(absmax, __shfl_xor_sync(0xffffffffu, absmax, offset));
    }
    const float scale = __fdiv_rn(absmax, 127.0f);
    const float inv = scale == 0.0f ? 0.0f : __fdiv_rn(1.0f, scale);
    const float scaled = __fmul_rn(value, inv);
    // roundf matches Rust f32::round (ties away from zero), whereas the
    // f16 scale uses ties to even. For tiny finite blocks inv can overflow:
    // Rust's saturating cast maps NaN (0 * inf) to zero and clamps infinities.
    const float quant = isnan(scaled) ? 0.0f : fminf(127.0f, fmaxf(-127.0f, roundf(scaled)));
    if (lane == 0) dst[block].d = __float2half_rn(scale);
    dst[block].qs[lane] = (int8_t)quant;
}

extern "C" cudaError_t makepad_cuda_quant_bf16_q8_0(
        const uint16_t *src, uint8_t *dst, uint32_t blocks,
        uint32_t *nonfinite, cudaStream_t stream) {
    if (!src || !dst || !blocks || !nonfinite) return cudaErrorInvalidValue;
    // The caller clears nonfinite on this stream and checks it before
    // publishing dst; no host weight staging is needed in either direction.
    q8_0_quantize_bf16_kernel<<<(blocks - 1) / 4 + 1, 128, 0, stream>>>(
        src, reinterpret_cast<block_q8_0 *>(dst), blocks, nonfinite);
    return cudaGetLastError();
}

// Match the existing bf16 f32-accumulating GEMM's activation staging
// (makepad_cuda_f32_to_bf16 in ops.cu), without a separate conversion launch.
static __device__ __forceinline__ float q8_activation(float x) {
    return __uint_as_float(__float_as_uint(x) & 0xffff0000u);
}

// Stage a whole 2048-column row at once (longer rows use successive
// tiles). This is load geometry, not an input-size limit. Lane ownership,
// the eight-value FMA chains, and the final shuffle tree stay unchanged.
static constexpr uint32_t Q8_TILE_BLOCKS = 64;
static constexpr uint32_t Q8_TILE_VECTORS = Q8_TILE_BLOCKS * 34 / 16;

template <int M, int ROWS>
static __device__ __forceinline__ void q8_0_dot_rows(
        const float *__restrict__ x, const block_q8_0 *__restrict__ weights,
        uint32_t row, uint32_t row_count, uint32_t k,
        uint4 (&tile)[ROWS][Q8_TILE_VECTORS], float (&sum)[ROWS][M]) {
    const uint32_t lane = threadIdx.x & 31;

    // Eight adjacent blocks occupy exactly seventeen 16-byte vectors.
    // Q8_0's 34-byte stride makes per-block vector loads unaligned. Stage
    // the contiguous byte stream instead: every weight byte is read once,
    // coalesced and aligned, then each group of four lanes consumes a block.
    const uint32_t blocks = k / 32;
    for (uint32_t first = 0; first < blocks; first += Q8_TILE_BLOCKS) {
        const uint32_t count = min(Q8_TILE_BLOCKS, blocks - first);
        #pragma unroll
        for (int r = 0; r < ROWS; ++r) {
            if (r >= row_count) continue;
            uint16_t *words = reinterpret_cast<uint16_t *>(tile[r]);
            const uint16_t *chunk = reinterpret_cast<const uint16_t *>(
                weights + (size_t)(row + r) * blocks + first);
            if (count % 8 == 0 && (reinterpret_cast<uintptr_t>(chunk) & 15u) == 0) {
                for (uint32_t i = lane; i < count / 8 * 17; i += 32) {
                    tile[r][i] = reinterpret_cast<const uint4 *>(chunk)[i];
                }
            } else {
                // General k % 32 == 0 tail/unaligned row. The decode matrices
                // with k % 256 == 0 exclusively take the vectorized path above.
                for (uint32_t i = lane; i < count * 17; i += 32) words[i] = chunk[i];
            }
        }
        __syncwarp();
        for (uint32_t b = 0; b < count; b += 8) {
            if (b + lane / 4 < count) {
                const uint32_t col = (first + b) * 32 + lane * 8;
                #pragma unroll
                for (int m = 0; m < M; ++m) {
                    const float4 a = *reinterpret_cast<const float4 *>(x + (size_t)m * k + col);
                    const float4 c = *reinterpret_cast<const float4 *>(x + (size_t)m * k + col + 4);
                    // Independent rows reuse these activations and expose
                    // independent FMA chains. Each row retains exactly the
                    // previous lane ownership, FMA order and shuffle tree.
                    #pragma unroll
                    for (int r = 0; r < ROWS; ++r) {
                        if (r >= row_count) continue;
                        const block_q8_0 *block = reinterpret_cast<const block_q8_0 *>(tile[r])
                            + b + lane / 4;
                        const float d = __half2float(block->d);
                        const int8_t *q = block->qs + (lane % 4) * 8;
                        sum[r][m] = fmaf(d * (float)q[0], q8_activation(a.x), sum[r][m]);
                        sum[r][m] = fmaf(d * (float)q[1], q8_activation(a.y), sum[r][m]);
                        sum[r][m] = fmaf(d * (float)q[2], q8_activation(a.z), sum[r][m]);
                        sum[r][m] = fmaf(d * (float)q[3], q8_activation(a.w), sum[r][m]);
                        sum[r][m] = fmaf(d * (float)q[4], q8_activation(c.x), sum[r][m]);
                        sum[r][m] = fmaf(d * (float)q[5], q8_activation(c.y), sum[r][m]);
                        sum[r][m] = fmaf(d * (float)q[6], q8_activation(c.z), sum[r][m]);
                        sum[r][m] = fmaf(d * (float)q[7], q8_activation(c.w), sum[r][m]);
                    }
                }
            }
        }
        __syncwarp(); // No lane may overwrite the next tile while peers read.
    }
    #pragma unroll
    for (int r = 0; r < ROWS; ++r) {
        #pragma unroll
        for (int m = 0; m < M; ++m) {
            #pragma unroll
            for (int offset = 16; offset > 0; offset >>= 1) {
                sum[r][m] += __shfl_down_sync(0xffffffffu, sum[r][m], offset);
            }
        }
    }
}

template <int M, int WARPS, bool SWIGLU = false, int ROWS = 1>
static __global__ void q8_0_gemv_kernel(
        const float *__restrict__ x, const block_q8_0 *__restrict__ weights,
        float *__restrict__ out, uint32_t k, uint32_t n, uint32_t out_stride) {
    const uint32_t warp = threadIdx.x >> 5;
    const uint32_t lane = threadIdx.x & 31;
    const uint32_t row = (blockIdx.x * WARPS + warp) * ROWS;
    __shared__ uint4 tile[WARPS][ROWS][Q8_TILE_VECTORS];
    if (row < n) {
        const uint32_t row_count = min((uint32_t)ROWS, n - row);
        float sum[ROWS][M] = {};
        q8_0_dot_rows<M, ROWS>(x, weights, row, row_count, k, tile[warp], sum);
        float gate[ROWS][M] = {};
        if (SWIGLU) q8_0_dot_rows<M, ROWS>(x, weights, n + row, row_count, k, tile[warp], gate);
        #pragma unroll
        for (int r = 0; r < ROWS; ++r) {
            #pragma unroll
            for (int m = 0; m < M; ++m) {
                if (lane == 0 && r < row_count) {
                    float value = sum[r][m];
                    if (SWIGLU) {
                        // Identical to swiglu_value_gate_f32, before the down
                        // GEMV truncates its activation to bf16 as usual.
                        const float s = gate[r][m] / (1.0f + expf(-gate[r][m]));
                        value = value * s;
                    }
                    out[(size_t)m * out_stride + row + r] = value;
                }
            }
        }
    }
}

// Aim for at least four CTAs per SM without changing any row's reduction.
// Small projections use all available rows; wider ones keep several rows
// per CTA. Query device properties only when the host switches devices.
static cudaError_t q8_0_launch_warps(uint32_t n, uint32_t *warps) {
    static thread_local int cached_device = -1, cached_sms = 0;
    int device;
    cudaError_t status = cudaGetDevice(&device);
    if (status != cudaSuccess) return status;
    if (device != cached_device) {
        status = cudaDeviceGetAttribute(&cached_sms, cudaDevAttrMultiProcessorCount, device);
        if (status != cudaSuccess) return status;
        cached_device = device;
    }
    const uint64_t target = (uint64_t)cached_sms * 4;
    *warps = ((uint64_t)n + 3) / 4 >= target ? 4 : ((uint64_t)n + 1) / 2 >= target ? 2 : 1;
    return cudaSuccess;
}

template <int M, bool SWIGLU = false>
static void q8_0_launch(const float *x, const block_q8_0 *w, float *out,
        uint32_t k, uint32_t n, uint32_t stride, uint32_t warps, cudaStream_t stream) {
    if (!SWIGLU && warps == 2 && k <= Q8_TILE_BLOCKS * 32) {
        // Two short rows per warp, one warp per CTA: the same ceil(n/2)
        // CTAs selected for >=4 per SM, with shared activation loads and
        // two independent accumulation chains instead of one. A 2048-row
        // projection launches 1024 CTAs. Large/fused projections keep the
        // existing geometry; one-row CTAs remain the small-n fallback.
        q8_0_gemv_kernel<M, 1, false, 2><<<(n - 1) / 2 + 1, 32, 0, stream>>>(x, w, out, k, n, stride);
    } else if (warps == 4) {
        q8_0_gemv_kernel<M, 4, SWIGLU><<<(n - 1) / 4 + 1, 128, 0, stream>>>(x, w, out, k, n, stride);
    } else if (warps == 2) {
        q8_0_gemv_kernel<M, 2, SWIGLU><<<(n - 1) / 2 + 1, 64, 0, stream>>>(x, w, out, k, n, stride);
    } else {
        q8_0_gemv_kernel<M, 1, SWIGLU><<<n, 32, 0, stream>>>(x, w, out, k, n, stride);
    }
}

extern "C" cudaError_t makepad_cuda_q8_0_gemv_f32(
        const float *x, const uint8_t *weights, float *out,
        uint32_t m, uint32_t k, uint32_t n, uint32_t out_stride, cudaStream_t stream) {
    if ((m != 1 && m != 2) || !k || k % 32 || !n || out_stride < n) {
        return cudaErrorInvalidValue;
    }
    uint32_t warps;
    cudaError_t status = q8_0_launch_warps(n, &warps);
    if (status != cudaSuccess) return status;
    MakepadLaunchTiming timing("q8.gemv", stream, n, k);
    const block_q8_0 *w = reinterpret_cast<const block_q8_0 *>(weights);
    if (m == 1) {
        q8_0_launch<1>(x, w, out, k, n, out_stride, warps, stream);
    } else {
        q8_0_launch<2>(x, w, out, k, n, out_stride, warps, stream);
    }
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_cuda_q8_0_gemv_swiglu_f32(
        const float *x, const uint8_t *weights, float *out,
        uint32_t k, uint32_t n, cudaStream_t stream) {
    if (!x || !weights || !out || !k || k % 32 || !n) return cudaErrorInvalidValue;
    uint32_t warps;
    cudaError_t status = q8_0_launch_warps(n, &warps);
    if (status != cudaSuccess) return status;
    MakepadLaunchTiming timing("q8.gemv_swiglu", stream, n, k);
    q8_0_launch<1, true>(x, reinterpret_cast<const block_q8_0 *>(weights), out, k, n, n, warps, stream);
    return cudaGetLastError();
}
