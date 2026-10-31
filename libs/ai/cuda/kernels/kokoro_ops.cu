#include <cuda_runtime.h>
#include <math.h>

// Direct convolution: each block stages only the input taps and weights for
// a 32-channel x 64-time output tile. No global patch matrix or transpose.
// Bias comes first, then input channels/taps in CPU order. Explicit rounded
// multiply/add prevents nvcc from contracting the CPU's two operations.
static __global__ void mkko_conv1d_kernel(
        const float * __restrict__ x, const float * __restrict__ w,
        const float * __restrict__ bias, float * __restrict__ y,
        int in_ch, int time, int out_ch, int kernel, int out_time,
        int pad, int stride, int dilation) {
    __shared__ float xs[32][64];
    __shared__ float ws[32][33];
    __shared__ bool valid[32][64];
    const int tx = threadIdx.x;
    const int ty = threadIdx.y;
    const int tid = ty * 16 + tx;
    const size_t time_tiles = (static_cast<size_t>(out_time) + 63) / 64;
    const size_t t_base = (blockIdx.x % time_tiles) * 64;
    const size_t oc_base = (blockIdx.x / time_tiles) * 32;
    const size_t width = static_cast<size_t>(in_ch) * kernel;
    float acc[4][4];
    #pragma unroll
    for (int i = 0; i < 4; ++i) {
        const size_t oc = oc_base + ty + i * 8;
        const float b = oc < out_ch ? bias[oc] : 0.0f;
        #pragma unroll
        for (int j = 0; j < 4; ++j) acc[i][j] = b;
    }
    for (size_t base = 0; base < width; base += 32) {
        for (int p = tid; p < 32 * 64; p += 128) {
            const int kk = p / 64;
            const int tt = p % 64;
            const size_t q = base + kk;
            const size_t t = t_base + tt;
            const size_t position = t * stride + (q % kernel) * dilation;
            const bool inside = q < width && t < out_time &&
                position >= pad && position - pad < time;
            valid[kk][tt] = inside;
            xs[kk][tt] = inside ? x[(q / kernel) * time + position - pad] : 0.0f;
        }
        for (int p = tid; p < 32 * 32; p += 128) {
            const int oc = p / 32;
            const int kk = p % 32;
            ws[oc][kk] = oc_base + oc < out_ch && base + kk < width
                ? w[(oc_base + oc) * width + base + kk] : 0.0f;
        }
        __syncthreads();
        #pragma unroll
        for (int kk = 0; kk < 32; ++kk) {
            float xv[4];
            bool inside[4];
            #pragma unroll
            for (int j = 0; j < 4; ++j) {
                xv[j] = xs[kk][tx + j * 16];
                inside[j] = valid[kk][tx + j * 16];
            }
            #pragma unroll
            for (int i = 0; i < 4; ++i) {
                const float tap = ws[ty + i * 8][kk];
                #pragma unroll
                for (int j = 0; j < 4; ++j) {
                    if (tap != 0.0f && inside[j]) {
                        acc[i][j] = __fadd_rn(acc[i][j], __fmul_rn(tap, xv[j]));
                    }
                }
            }
        }
        __syncthreads();
    }
    #pragma unroll
    for (int i = 0; i < 4; ++i) {
        const size_t oc = oc_base + ty + i * 8;
        #pragma unroll
        for (int j = 0; j < 4; ++j) {
            const size_t t = t_base + tx + j * 16;
            if (oc < out_ch && t < out_time) y[oc * out_time + t] = acc[i][j];
        }
    }
}

// One f32 GEMM for every timestep's input projection, with both biases
// folded into the initial accumulator, just like LstmLayer::run. Tiling
// reuses loads without changing the reduction order or using TF32/FMA.
static __global__ void mkko_lstm_input_kernel(
        const float * __restrict__ x, const float * __restrict__ w,
        const float * __restrict__ bias_ih, const float * __restrict__ bias_hh,
        float * __restrict__ gates, int time, int input, int gate_count) {
    __shared__ float xs[64][33];
    __shared__ float ws[64][33];
    const int tx = threadIdx.x;
    const int ty = threadIdx.y;
    const int tid = ty * 16 + tx;
    const size_t gate_tiles = (static_cast<size_t>(gate_count) + 63) / 64;
    const size_t t_base = (blockIdx.x / gate_tiles) * 64;
    const size_t g_base = (blockIdx.x % gate_tiles) * 64;
    float acc[4][4];
    #pragma unroll
    for (int j = 0; j < 4; ++j) {
        const size_t g = g_base + tx + j * 16;
        const float b = g < gate_count ? __fadd_rn(bias_ih[g], bias_hh[g]) : 0.0f;
        #pragma unroll
        for (int i = 0; i < 4; ++i) acc[i][j] = b;
    }
    for (size_t base = 0; base < static_cast<size_t>(input); base += 32) {
        for (int p = tid; p < 64 * 32; p += 256) {
            const int row = p / 32;
            const size_t k = base + p % 32;
            xs[row][p % 32] = t_base + row < time && k < input
                ? x[(t_base + row) * input + k] : 0.0f;
            ws[row][p % 32] = g_base + row < gate_count && k < input
                ? w[(g_base + row) * input + k] : 0.0f;
        }
        __syncthreads();
        #pragma unroll
        for (int kk = 0; kk < 32; ++kk) {
            if (base + kk < input) {
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
                        acc[i][j] = __fadd_rn(acc[i][j], __fmul_rn(wv[j], xv[i]));
                    }
                }
            }
        }
        __syncthreads();
    }
    #pragma unroll
    for (int i = 0; i < 4; ++i) {
        const size_t t = t_base + ty + i * 16;
        #pragma unroll
        for (int j = 0; j < 4; ++j) {
            const size_t g = g_base + tx + j * 16;
            if (t < time && g < gate_count) gates[t * gate_count + g] = acc[i][j];
        }
    }
}

static __device__ __forceinline__ float mkko_sigmoid(float x) {
    return __fdiv_rn(1.0f, __fadd_rn(1.0f, expf(-x)));
}

// One warp per hidden unit coalesces reads of all four recurrent rows.
// Feed products to the accumulators in column order via shuffles, rather
// than a tree reduction, preserving the CPU's sequential f32 additions.
// Previous h is a completed output row; the new row is disjoint. A launch
// boundary on the same stream supplies the inter-timestep grid barrier.
static __global__ void mkko_lstm_step_kernel(
        const float * __restrict__ gates, const float * __restrict__ w,
        const float * __restrict__ previous, float * __restrict__ c,
        float * __restrict__ y, int hidden) {
    const int lane = threadIdx.x & 31;
    const size_t j = static_cast<size_t>(blockIdx.x) * 4 + (threadIdx.x >> 5);
    if (j >= hidden) return;
    float sum[4];
    #pragma unroll
    for (int g = 0; g < 4; ++g) sum[g] = gates[static_cast<size_t>(g) * hidden + j];
    for (size_t base = 0; base < static_cast<size_t>(hidden); base += 32) {
        const size_t k = base + lane;
        const float h = previous && k < hidden ? previous[k] : 0.0f;
        float product[4];
        #pragma unroll
        for (int g = 0; g < 4; ++g) {
            product[g] = k < hidden
                ? __fmul_rn(w[(static_cast<size_t>(g) * hidden + j) * hidden + k], h)
                : 0.0f;
        }
        #pragma unroll
        for (int col = 0; col < 32; ++col) {
            if (base + col < hidden) {
                #pragma unroll
                for (int g = 0; g < 4; ++g) {
                    sum[g] = __fadd_rn(sum[g], __shfl_sync(0xffffffffu, product[g], col));
                }
            }
        }
    }
    if (lane == 0) {
        const float i = mkko_sigmoid(sum[0]);
        const float f = mkko_sigmoid(sum[1]);
        const float g = tanhf(sum[2]);
        const float o = mkko_sigmoid(sum[3]);
        const float cell = __fadd_rn(__fmul_rn(f, previous ? c[j] : 0.0f), __fmul_rn(i, g));
        c[j] = cell;
        y[j] = __fmul_rn(o, tanhf(cell));
    }
}

extern "C" cudaError_t mkko_conv1d(
        const float * x, const float * w, const float * bias, float * y,
        int in_ch, int time, int out_ch, int kernel, int out_time,
        int pad, int stride, int dilation, cudaStream_t stream) {
    if (in_ch <= 0 || time < 0 || out_ch <= 0 || kernel <= 0 || out_time <= 0 ||
            pad < 0 || stride <= 0 || dilation <= 0) return cudaErrorInvalidValue;
    const size_t blocks = ((static_cast<size_t>(out_ch) + 31) / 32) *
        ((static_cast<size_t>(out_time) + 63) / 64);
    if (blocks > 0x7fffffffu) return cudaErrorInvalidValue;
    mkko_conv1d_kernel<<<static_cast<unsigned int>(blocks), dim3(16, 8), 0, stream>>>(
        x, w, bias, y, in_ch, time, out_ch, kernel, out_time, pad, stride, dilation);
    return cudaGetLastError();
}

extern "C" cudaError_t mkko_lstm_input(
        const float * x, const float * w, const float * bias_ih, const float * bias_hh,
        float * gates, int time, int input, int hidden, cudaStream_t stream) {
    if (time < 0 || input <= 0 || hidden <= 0 || hidden > 0x7fffffff / 4)
        return cudaErrorInvalidValue;
    if (time == 0) return cudaSuccess;
    const int gate_count = 4 * hidden;
    const size_t blocks = ((static_cast<size_t>(time) + 63) / 64) *
        ((static_cast<size_t>(gate_count) + 63) / 64);
    if (blocks > 0x7fffffffu) return cudaErrorInvalidValue;
    mkko_lstm_input_kernel<<<static_cast<unsigned int>(blocks), dim3(16, 16), 0, stream>>>(
        x, w, bias_ih, bias_hh, gates, time, input, gate_count);
    return cudaGetLastError();
}

extern "C" cudaError_t mkko_lstm_recurrent(
        const float * gates, const float * w, float * c, float * y,
        int time, int hidden, int reverse, cudaStream_t stream) {
    if (time < 0 || hidden <= 0 || hidden > 0x7fffffff / 4) return cudaErrorInvalidValue;
    const unsigned int blocks = (static_cast<unsigned int>(hidden) + 3) / 4;
    const float * previous = nullptr;
    for (int step = 0; step < time; ++step) {
        const int t = reverse ? time - 1 - step : step;
        float * row = y + static_cast<size_t>(t) * hidden;
        mkko_lstm_step_kernel<<<blocks, 128, 0, stream>>>(
            gates + static_cast<size_t>(t) * 4 * hidden, w, previous, c, row, hidden);
        const cudaError_t status = cudaGetLastError();
        if (status != cudaSuccess) return status;
        previous = row;
    }
    return cudaGetLastError();
}
