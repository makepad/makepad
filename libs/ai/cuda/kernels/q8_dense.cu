#include <cuda_fp16.h>
#include <cuda_runtime.h>
#include <stdint.h>

// Q8_0 blocks are a little-endian f16 scale followed by 32 signed bytes.
// Read the scale as bytes: blocks and rows need not be 4-byte aligned.
static __device__ __forceinline__ float mkq8_scale(const uint8_t * block) {
    const uint16_t bits = static_cast<uint16_t>(block[0]) |
        (static_cast<uint16_t>(block[1]) << 8);
    return __half2float(*reinterpret_cast<const __half *>(&bits));
}

// One warp per output row: lanes read consecutive activation/weight values.
static __global__ void mkq8_gemv_nt_kernel(
        const float * __restrict__ x,
        const uint8_t * __restrict__ w,
        float * __restrict__ y,
        int k,
        int n) {
    const int lane = threadIdx.x & 31;
    const int row = static_cast<int>(blockIdx.x) * 4 + (threadIdx.x >> 5);
    if (row >= n) {
        return;
    }
    const int blocks_per_row = k / 32;
    const size_t row_offset = static_cast<size_t>(row) * blocks_per_row * 34;
    float sum = 0.0f;
    #pragma unroll 4
    for (int b = 0; b < blocks_per_row; ++b) {
        const uint8_t * block = w + row_offset + static_cast<size_t>(b) * 34;
        float d = lane == 0 ? mkq8_scale(block) : 0.0f;
        d = __shfl_sync(0xffffffffu, d, 0);
        const float q = static_cast<float>(reinterpret_cast<const int8_t *>(block + 2)[lane]);
        sum = fmaf(x[b * 32 + lane], d * q, sum);
    }
    #pragma unroll
    for (int offset = 16; offset > 0; offset >>= 1) {
        sum += __shfl_down_sync(0xffffffffu, sum, offset);
    }
    if (lane == 0) {
        y[row] = sum;
    }
}

// A 16x16 thread block computes a 64x64 output tile, four rows and four
// columns per thread. Only the current 32-wide K tile is dequantized, in
// shared memory; the resident weight allocation remains raw Q8_0 bytes.
static __global__ void mkq8_gemm_nt_kernel(
        const float * __restrict__ x,
        const uint8_t * __restrict__ w,
        float * __restrict__ y,
        int m,
        int k,
        int n) {
    // Padding avoids bank conflicts when the compute phase reads columns.
    __shared__ float xs[64][33];
    __shared__ float ws[64][33];
    const int tx = threadIdx.x;
    const int ty = threadIdx.y;
    const int tid = ty * 16 + tx;
    const int lane = tid & 31;
    const int warp = tid >> 5;
    const int row_base = static_cast<int>(blockIdx.y) * 64;
    const int col_base = static_cast<int>(blockIdx.x) * 64;
    const int blocks_per_row = k / 32;
    float acc[4][4] = {};

    for (int b = 0; b < blocks_per_row; ++b) {
        // Each warp stages complete rows with coalesced loads. All lanes
        // take the same weight-row guard, including the scale shuffle.
        #pragma unroll
        for (int r = warp; r < 64; r += 8) {
            const int row = row_base + r;
            xs[r][lane] = row < m ? x[static_cast<size_t>(row) * k + b * 32 + lane] : 0.0f;
            const int col = col_base + r;
            float weight = 0.0f;
            if (col < n) {
                const uint8_t * block = w +
                    (static_cast<size_t>(col) * blocks_per_row + b) * 34;
                float d = lane == 0 ? mkq8_scale(block) : 0.0f;
                d = __shfl_sync(0xffffffffu, d, 0);
                weight = d * static_cast<float>(reinterpret_cast<const int8_t *>(block + 2)[lane]);
            }
            ws[r][lane] = weight;
        }
        __syncthreads();

        #pragma unroll
        for (int kk = 0; kk < 32; ++kk) {
            float xv[4];
            float wv[4];
            #pragma unroll
            for (int i = 0; i < 4; ++i) {
                xv[i] = xs[ty + i * 16][kk];
                wv[i] = ws[tx + i * 16][kk];
            }
            #pragma unroll
            for (int i = 0; i < 4; ++i) {
                #pragma unroll
                for (int j = 0; j < 4; ++j) {
                    acc[i][j] = fmaf(xv[i], wv[j], acc[i][j]);
                }
            }
        }
        __syncthreads();
    }

    #pragma unroll
    for (int i = 0; i < 4; ++i) {
        const int row = row_base + ty + i * 16;
        #pragma unroll
        for (int j = 0; j < 4; ++j) {
            const int col = col_base + tx + j * 16;
            if (row < m && col < n) {
                y[static_cast<size_t>(row) * n + col] = acc[i][j];
            }
        }
    }
}

extern "C" cudaError_t mkq8_gemm_nt(
        const float * x,
        const uint8_t * w,
        float * y,
        int m,
        int k,
        int n,
        cudaStream_t stream) {
    if (m < 0 || k < 0 || n < 0 || k % 32 != 0) {
        return cudaErrorInvalidValue;
    }
    if (m == 0 || n == 0) {
        return cudaSuccess;
    }
    if (m == 1) {
        mkq8_gemv_nt_kernel<<<(n - 1) / 4 + 1, 128, 0, stream>>>(x, w, y, k, n);
    } else {
        const dim3 block(16, 16, 1);
        const dim3 grid((n - 1) / 64 + 1, (m - 1) / 64 + 1, 1);
        mkq8_gemm_nt_kernel<<<grid, block, 0, stream>>>(x, w, y, m, k, n);
    }
    return cudaGetLastError();
}
