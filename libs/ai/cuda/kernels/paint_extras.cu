// Focused Hunyuan3D-Paint extras kernels: wide-V reference attention and
// batched independent self-attention. Do not fold these into diffusion_ops.cu.
#include <cuda_runtime.h>
#include <math.h>
#include <stdint.h>

static constexpr int kPaintMaxHeadDim = 128;

// Matrix transpose: [rows, cols] -> [cols, rows]. Planar [C, HW] ↔ tokens [HW, C].
static __global__ void makepad_ggml_cuda_paint_transpose_f32_kernel(
    const float * __restrict__ input,
    float * __restrict__ output,
    uint32_t rows,
    uint32_t cols
) {
    const size_t i = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    const size_t n = static_cast<size_t>(rows) * cols;
    if (i >= n) {
        return;
    }
    const uint32_t r = static_cast<uint32_t>(i / cols);
    const uint32_t c = static_cast<uint32_t>(i - static_cast<size_t>(r) * cols);
    output[static_cast<size_t>(c) * rows + r] = input[i];
}

extern "C" cudaError_t makepad_ggml_cuda_paint_transpose_f32(
    const float * input,
    float * output,
    uint32_t rows,
    uint32_t cols,
    cudaStream_t stream
) {
    const size_t n = static_cast<size_t>(rows) * cols;
    if (n == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<unsigned int>((n + block.x - 1) / block.x), 1, 1);
    makepad_ggml_cuda_paint_transpose_f32_kernel<<<grid, block, 0, stream>>>(
        input, output, rows, cols);
    return cudaGetLastError();
}

static __device__ __forceinline__ float paint_dot(
    const float * __restrict__ a,
    const float * __restrict__ b,
    uint32_t n
) {
    float acc = 0.0f;
    for (uint32_t i = 0; i < n; ++i) {
        acc += a[i] * b[i];
    }
    return acc;
}

// Official RA: V is cat(v_alb, v_mr) on the last dim, then viewed as
// [heads, 2*head_dim]. Matches libs/pbr_paint/src/unet_extras.rs.
static __global__ void makepad_ggml_cuda_paint_ref_attn_wide_v_f32_kernel(
    const float * __restrict__ q,
    const float * __restrict__ k,
    const float * __restrict__ v_alb,
    const float * __restrict__ v_mr,
    float * __restrict__ o_alb,
    float * __restrict__ o_mr,
    uint32_t q_len,
    uint32_t kv_len,
    uint32_t hidden,
    uint32_t heads,
    float scale
) {
    const uint32_t qi = blockIdx.x * blockDim.x + threadIdx.x;
    const uint32_t h = blockIdx.y;
    if (qi >= q_len || h >= heads) {
        return;
    }
    const uint32_t head_dim = hidden / heads;
    const uint32_t v_head = head_dim * 2;
    if (head_dim == 0 || head_dim > kPaintMaxHeadDim) {
        return;
    }
    const float * qh = q + static_cast<size_t>(qi) * hidden + static_cast<size_t>(h) * head_dim;

    float m = -INFINITY;
    float l = 0.0f;
    float acc[kPaintMaxHeadDim * 2];
    for (uint32_t d = 0; d < v_head; ++d) {
        acc[d] = 0.0f;
    }
    for (uint32_t ki = 0; ki < kv_len; ++ki) {
        const float * kh = k + static_cast<size_t>(ki) * hidden + static_cast<size_t>(h) * head_dim;
        const float s = paint_dot(qh, kh, head_dim) * scale;
        const float m2 = fmaxf(m, s);
        const float alpha = __expf(m - m2);
        const float p = __expf(s - m2);
        for (uint32_t d = 0; d < v_head; ++d) {
            acc[d] *= alpha;
        }
        const uint32_t pack0 = h * v_head;
        for (uint32_t d = 0; d < v_head; ++d) {
            const uint32_t pack = pack0 + d;
            const float * src = pack < hidden ? v_alb : v_mr;
            const uint32_t off = pack < hidden ? pack : pack - hidden;
            acc[d] += p * src[static_cast<size_t>(ki) * hidden + off];
        }
        l = l * alpha + p;
        m = m2;
    }
    const float inv = 1.0f / fmaxf(l, 1e-12f);
    float * oa = o_alb + static_cast<size_t>(qi) * hidden + static_cast<size_t>(h) * head_dim;
    float * om = o_mr + static_cast<size_t>(qi) * hidden + static_cast<size_t>(h) * head_dim;
    for (uint32_t d = 0; d < head_dim; ++d) {
        oa[d] = acc[d] * inv;
        om[d] = acc[head_dim + d] * inv;
    }
}

extern "C" cudaError_t makepad_ggml_cuda_paint_ref_attn_wide_v_f32(
    const float * q,
    const float * k,
    const float * v_alb,
    const float * v_mr,
    float * o_alb,
    float * o_mr,
    uint32_t q_len,
    uint32_t kv_len,
    uint32_t hidden,
    uint32_t heads,
    float scale,
    cudaStream_t stream
) {
    if (q_len == 0 || kv_len == 0 || heads == 0 || hidden == 0) {
        return cudaSuccess;
    }
    if (hidden % heads != 0 || (hidden / heads) > kPaintMaxHeadDim) {
        return cudaErrorInvalidValue;
    }
    const dim3 block(128, 1, 1);
    const dim3 grid((q_len + block.x - 1) / block.x, heads, 1);
    makepad_ggml_cuda_paint_ref_attn_wide_v_f32_kernel<<<grid, block, 0, stream>>>(
        q, k, v_alb, v_mr, o_alb, o_mr, q_len, kv_len, hidden, heads, scale);
    return cudaGetLastError();
}

// Independent self-attn over `batch` sequences packed as [batch * seq, hidden].
static __global__ void makepad_ggml_cuda_paint_attn_batched_self_f32_kernel(
    const float * __restrict__ q,
    const float * __restrict__ k,
    const float * __restrict__ v,
    float * __restrict__ out,
    uint32_t batch,
    uint32_t seq,
    uint32_t hidden,
    uint32_t heads,
    float scale
) {
    const uint32_t qi = blockIdx.x * blockDim.x + threadIdx.x;
    const uint32_t bh = blockIdx.y;
    const uint32_t b = bh / heads;
    const uint32_t h = bh - b * heads;
    if (qi >= seq || b >= batch || h >= heads) {
        return;
    }
    const uint32_t head_dim = hidden / heads;
    if (head_dim == 0 || head_dim > kPaintMaxHeadDim) {
        return;
    }
    const size_t row0 = (static_cast<size_t>(b) * seq + qi) * hidden + static_cast<size_t>(h) * head_dim;
    const float * qh = q + row0;
    float m = -INFINITY;
    float l = 0.0f;
    float acc[kPaintMaxHeadDim];
    for (uint32_t d = 0; d < head_dim; ++d) {
        acc[d] = 0.0f;
    }
    const size_t batch_off = static_cast<size_t>(b) * seq * hidden;
    for (uint32_t ki = 0; ki < seq; ++ki) {
        const float * kh = k + batch_off + (static_cast<size_t>(ki) * hidden + static_cast<size_t>(h) * head_dim);
        const float s = paint_dot(qh, kh, head_dim) * scale;
        const float m2 = fmaxf(m, s);
        const float alpha = __expf(m - m2);
        const float p = __expf(s - m2);
        const float * vh = v + batch_off + (static_cast<size_t>(ki) * hidden + static_cast<size_t>(h) * head_dim);
        for (uint32_t d = 0; d < head_dim; ++d) {
            acc[d] = acc[d] * alpha + p * vh[d];
        }
        l = l * alpha + p;
        m = m2;
    }
    const float inv = 1.0f / fmaxf(l, 1e-12f);
    float * oh = out + row0;
    for (uint32_t d = 0; d < head_dim; ++d) {
        oh[d] = acc[d] * inv;
    }
}

static __global__ void makepad_ggml_cuda_paint_scale_f32_kernel(
    const float * __restrict__ input,
    float * __restrict__ output,
    float scale,
    uint32_t n
) {
    const uint32_t i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i < n) {
        output[i] = input[i] * scale;
    }
}

extern "C" cudaError_t makepad_ggml_cuda_paint_scale_f32(
    const float * input,
    float * output,
    float scale,
    uint32_t n,
    cudaStream_t stream
) {
    if (n == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid((n + block.x - 1) / block.x, 1, 1);
    makepad_ggml_cuda_paint_scale_f32_kernel<<<grid, block, 0, stream>>>(input, output, scale, n);
    return cudaGetLastError();
}

// 3D PoseRoPE matching libs/pbr_paint apply_pose_rope: per-head rotary with
// xy dim = head_dim/8*3 and z dim = head_dim/8*2, theta=10000.
static __global__ void makepad_ggml_cuda_paint_pose_rope_f32_kernel(
    const float * __restrict__ x,
    const uint32_t * __restrict__ xyz,
    float * __restrict__ out,
    uint32_t seq,
    uint32_t hidden,
    uint32_t heads,
    uint32_t voxel_res
) {
    const uint32_t s = blockIdx.x;
    const uint32_t h = blockIdx.y;
    const uint32_t pair = threadIdx.x;
    if (s >= seq || h >= heads) {
        return;
    }
    const uint32_t head_dim = hidden / heads;
    const uint32_t pairs = head_dim / 2;
    if (pair >= pairs) {
        return;
    }
    const uint32_t dim_xy = (head_dim / 8) * 3;
    const uint32_t dim_z = (head_dim / 8) * 2;
    const uint32_t pairs_xy = dim_xy / 2;
    const uint32_t pairs_z = dim_z / 2;
    uint32_t axis_dim = dim_xy;
    uint32_t axis = 0;
    uint32_t local = pair;
    if (pair < pairs_xy) {
        axis = 0;
        local = pair;
        axis_dim = dim_xy;
    } else if (pair < pairs_xy * 2) {
        axis = 1;
        local = pair - pairs_xy;
        axis_dim = dim_xy;
    } else {
        axis = 2;
        local = pair - pairs_xy * 2;
        axis_dim = dim_z;
        if (local >= pairs_z) {
            return;
        }
    }
    uint32_t pos = xyz[s * 3 + axis];
    if (voxel_res > 0 && pos >= voxel_res) {
        pos = voxel_res - 1;
    }
    const float freq = expf(-logf(10000.0f) * (2.0f * static_cast<float>(local)) / static_cast<float>(axis_dim));
    const float a = static_cast<float>(pos) * freq;
    const float c = cosf(a);
    const float si = sinf(a);
    const size_t base = (static_cast<size_t>(s) * hidden + static_cast<size_t>(h) * head_dim) + static_cast<size_t>(pair) * 2;
    const float re = x[base];
    const float im = x[base + 1];
    out[base] = re * c - im * si;
    out[base + 1] = im * c + re * si;
}

extern "C" cudaError_t makepad_ggml_cuda_paint_pose_rope_f32(
    const float * x,
    const uint32_t * xyz,
    float * out,
    uint32_t seq,
    uint32_t hidden,
    uint32_t heads,
    uint32_t voxel_res,
    cudaStream_t stream
) {
    if (seq == 0 || hidden == 0 || heads == 0) {
        return cudaSuccess;
    }
    if (hidden % heads != 0 || (hidden / heads) % 2 != 0) {
        return cudaErrorInvalidValue;
    }
    const uint32_t pairs = (hidden / heads) / 2;
    const dim3 block(pairs < 128 ? pairs : 128, 1, 1);
    const dim3 grid(seq, heads, 1);
    makepad_ggml_cuda_paint_pose_rope_f32_kernel<<<grid, block, 0, stream>>>(
        x, xyz, out, seq, hidden, heads, voxel_res);
    return cudaGetLastError();
}

// [batch * seq, heads * hd] token-major -> [batch * heads * seq, hd]
// so one strided-batched GEMM can run over batch*heads.
static __global__ void makepad_ggml_cuda_paint_pack_heads_f32_kernel(
    const float * __restrict__ input,
    float * __restrict__ output,
    uint32_t batch,
    uint32_t seq,
    uint32_t heads,
    uint32_t head_dim
) {
    const size_t n = static_cast<size_t>(batch) * seq * heads * head_dim;
    const size_t i = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    if (i >= n) {
        return;
    }
    const uint32_t hidden = heads * head_dim;
    const uint32_t d = static_cast<uint32_t>(i % head_dim);
    const size_t t = i / head_dim;
    const uint32_t h = static_cast<uint32_t>(t % heads);
    const size_t bs = t / heads;
    const uint32_t s = static_cast<uint32_t>(bs % seq);
    const uint32_t b = static_cast<uint32_t>(bs / seq);
    const size_t src = (static_cast<size_t>(b) * seq + s) * hidden + static_cast<size_t>(h) * head_dim + d;
    const size_t dst = ((static_cast<size_t>(b) * heads + h) * seq + s) * head_dim + d;
    output[dst] = input[src];
}

extern "C" cudaError_t makepad_ggml_cuda_paint_pack_heads_f32(
    const float * input,
    float * output,
    uint32_t batch,
    uint32_t seq,
    uint32_t heads,
    uint32_t head_dim,
    cudaStream_t stream
) {
    const size_t n = static_cast<size_t>(batch) * seq * heads * head_dim;
    if (n == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<unsigned int>((n + block.x - 1) / block.x), 1, 1);
    makepad_ggml_cuda_paint_pack_heads_f32_kernel<<<grid, block, 0, stream>>>(
        input, output, batch, seq, heads, head_dim);
    return cudaGetLastError();
}

static __global__ void makepad_ggml_cuda_paint_unpack_heads_f32_kernel(
    const float * __restrict__ input,
    float * __restrict__ output,
    uint32_t batch,
    uint32_t seq,
    uint32_t heads,
    uint32_t head_dim
) {
    const size_t n = static_cast<size_t>(batch) * seq * heads * head_dim;
    const size_t i = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    if (i >= n) {
        return;
    }
    const uint32_t hidden = heads * head_dim;
    const uint32_t d = static_cast<uint32_t>(i % head_dim);
    const size_t t = i / head_dim;
    const uint32_t s = static_cast<uint32_t>(t % seq);
    const size_t bh = t / seq;
    const uint32_t h = static_cast<uint32_t>(bh % heads);
    const uint32_t b = static_cast<uint32_t>(bh / heads);
    const size_t src = ((static_cast<size_t>(b) * heads + h) * seq + s) * head_dim + d;
    const size_t dst = (static_cast<size_t>(b) * seq + s) * hidden + static_cast<size_t>(h) * head_dim + d;
    output[dst] = input[src];
}

extern "C" cudaError_t makepad_ggml_cuda_paint_unpack_heads_f32(
    const float * input,
    float * output,
    uint32_t batch,
    uint32_t seq,
    uint32_t heads,
    uint32_t head_dim,
    cudaStream_t stream
) {
    const size_t n = static_cast<size_t>(batch) * seq * heads * head_dim;
    if (n == 0) {
        return cudaSuccess;
    }
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<unsigned int>((n + block.x - 1) / block.x), 1, 1);
    makepad_ggml_cuda_paint_unpack_heads_f32_kernel<<<grid, block, 0, stream>>>(
        input, output, batch, seq, heads, head_dim);
    return cudaGetLastError();
}

// Independent GroupNorm over N planar images packed as [C, N * H * W].
static __global__ void makepad_ggml_cuda_paint_gn_batched_stats_f32_kernel(
    const float * __restrict__ input,
    float * __restrict__ stats,
    uint32_t width,
    uint32_t height,
    uint32_t channels,
    uint32_t groups,
    uint32_t batch,
    float eps
) {
    const uint32_t g = blockIdx.x;
    const uint32_t b = blockIdx.y;
    if (g >= groups || b >= batch) {
        return;
    }
    const uint32_t cpg = channels / groups;
    const uint32_t plane = width * height;
    const uint32_t c0 = g * cpg;
    const size_t col0 = static_cast<size_t>(b) * plane;
    const size_t count = static_cast<size_t>(cpg) * plane;
    double sum = 0.0;
    double sumsq = 0.0;
    for (uint32_t c = 0; c < cpg; ++c) {
        const float * row = input + static_cast<size_t>(c0 + c) * (static_cast<size_t>(batch) * plane) + col0;
        for (uint32_t i = threadIdx.x; i < plane; i += blockDim.x) {
            const double v = static_cast<double>(row[i]);
            sum += v;
            sumsq += v * v;
        }
    }
    for (int offset = 16; offset > 0; offset >>= 1) {
        sum += __shfl_down_sync(0xffffffffu, sum, offset);
        sumsq += __shfl_down_sync(0xffffffffu, sumsq, offset);
    }
    __shared__ double sh_sum[8];
    __shared__ double sh_sumsq[8];
    const int warp = threadIdx.x >> 5;
    const int lane = threadIdx.x & 31;
    if (lane == 0) {
        sh_sum[warp] = sum;
        sh_sumsq[warp] = sumsq;
    }
    __syncthreads();
    if (threadIdx.x == 0) {
        double t = 0.0;
        double t2 = 0.0;
        const int warps = (blockDim.x + 31) >> 5;
        for (int w = 0; w < warps; ++w) {
            t += sh_sum[w];
            t2 += sh_sumsq[w];
        }
        const double mean = t / static_cast<double>(count);
        const double var = fmax(t2 / static_cast<double>(count) - mean * mean, 0.0);
        const uint32_t idx = b * groups + g;
        stats[idx * 2] = static_cast<float>(mean);
        stats[idx * 2 + 1] = static_cast<float>(rsqrt(var + static_cast<double>(eps)));
    }
}

static __global__ void makepad_ggml_cuda_paint_gn_batched_apply_f32_kernel(
    const float * __restrict__ input,
    const float * __restrict__ gamma,
    const float * __restrict__ beta,
    const float * __restrict__ stats,
    float * __restrict__ output,
    uint32_t width,
    uint32_t height,
    uint32_t channels,
    uint32_t groups,
    uint32_t batch
) {
    const size_t n = static_cast<size_t>(channels) * batch * width * height;
    const size_t i = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    if (i >= n) {
        return;
    }
    const uint32_t plane = width * height;
    const uint32_t cols = batch * plane;
    const uint32_t col = static_cast<uint32_t>(i % cols);
    const uint32_t c = static_cast<uint32_t>(i / cols);
    const uint32_t b = col / plane;
    const uint32_t cpg = channels / groups;
    const uint32_t g = c / cpg;
    const uint32_t idx = b * groups + g;
    const float mean = stats[idx * 2];
    const float inv = stats[idx * 2 + 1];
    const float x = input[i];
    output[i] = (x - mean) * inv * gamma[c] + beta[c];
}

extern "C" cudaError_t makepad_ggml_cuda_paint_gn_batched_f32(
    const float * input,
    const float * gamma,
    const float * beta,
    float * stats,
    float * output,
    uint32_t width,
    uint32_t height,
    uint32_t channels,
    uint32_t groups,
    uint32_t batch,
    float eps,
    cudaStream_t stream
) {
    if (batch == 0 || width == 0 || height == 0 || channels == 0 || groups == 0) {
        return cudaSuccess;
    }
    if (channels % groups != 0) {
        return cudaErrorInvalidValue;
    }
    const dim3 sblock(256, 1, 1);
    const dim3 sgrid(groups, batch, 1);
    makepad_ggml_cuda_paint_gn_batched_stats_f32_kernel<<<sgrid, sblock, 0, stream>>>(
        input, stats, width, height, channels, groups, batch, eps);
    cudaError_t err = cudaGetLastError();
    if (err != cudaSuccess) {
        return err;
    }
    const size_t n = static_cast<size_t>(channels) * batch * width * height;
    const dim3 ablock(256, 1, 1);
    const dim3 agrid(static_cast<unsigned int>((n + ablock.x - 1) / ablock.x), 1, 1);
    makepad_ggml_cuda_paint_gn_batched_apply_f32_kernel<<<agrid, ablock, 0, stream>>>(
        input, gamma, beta, stats, output, width, height, channels, groups, batch);
    return cudaGetLastError();
}

extern "C" cudaError_t makepad_ggml_cuda_paint_attn_batched_self_f32(
    const float * q,
    const float * k,
    const float * v,
    float * out,
    uint32_t batch,
    uint32_t seq,
    uint32_t hidden,
    uint32_t heads,
    float scale,
    cudaStream_t stream
) {
    if (batch == 0 || seq == 0 || heads == 0 || hidden == 0) {
        return cudaSuccess;
    }
    if (hidden % heads != 0 || (hidden / heads) > kPaintMaxHeadDim) {
        return cudaErrorInvalidValue;
    }
    const dim3 block(128, 1, 1);
    const dim3 grid((seq + block.x - 1) / block.x, batch * heads, 1);
    makepad_ggml_cuda_paint_attn_batched_self_f32_kernel<<<grid, block, 0, stream>>>(
        q, k, v, out, batch, seq, hidden, heads, scale);
    return cudaGetLastError();
}
