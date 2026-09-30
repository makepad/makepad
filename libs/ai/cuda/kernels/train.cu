// Training kernels: forward and backward of the small op set the singing
// model (libs/ai/models/sing, nn.rs) is built from, plus the optimiser,
// the STFT family and monotonic alignment. f32 row-major [rows, cols]
// tensors; "seg" is the number of rows per batch item (convolutions, RoPE
// positions and attention never cross an item). Every *_bwd kernel
// ACCUMULATES into its gradient buffer (+=). Launch wrappers are
// `extern "C" cudaError_t mkt_*(..., cudaStream_t)`.

#include <cuda_runtime.h>
#include <stdint.h>
#include <math.h>

#define MKT_THREADS 256

static inline unsigned mkt_blocks(size_t n) {
    size_t b = (n + MKT_THREADS - 1) / MKT_THREADS;
    if (b > 65535u * 8u) b = 65535u * 8u;
    return (unsigned)(b ? b : 1);
}

#define GRID_LOOP(i, n) for (size_t i = blockIdx.x * (size_t)blockDim.x + threadIdx.x; i < (n); i += (size_t)blockDim.x * gridDim.x)

// ---------------------------------------------------------------------------
// Activations: 0 gelu(tanh), 1 silu, 2 relu, 3 leaky relu 0.1, 4 tanh,
// 5 sigmoid, 6 exp(min(x, 10))
// ---------------------------------------------------------------------------

__device__ __forceinline__ float act_f(int op, float x) {
    switch (op) {
    case 0: return 0.5f * x * (1.0f + tanhf(0.7978846f * (x + 0.044715f * x * x * x)));
    case 1: return x / (1.0f + __expf(-x));
    case 2: return fmaxf(x, 0.0f);
    case 3: return x > 0.0f ? x : 0.1f * x;
    case 4: return tanhf(x);
    case 5: return 1.0f / (1.0f + __expf(-x));
    default: return __expf(fminf(x, 10.0f));
    }
}

__device__ __forceinline__ float act_g(int op, float x) {
    switch (op) {
    case 0: {
        float u = 0.7978846f * (x + 0.044715f * x * x * x);
        float t = tanhf(u);
        return 0.5f * (1.0f + t) + 0.5f * x * (1.0f - t * t) * 0.7978846f * (1.0f + 3.0f * 0.044715f * x * x);
    }
    case 1: { float s = 1.0f / (1.0f + __expf(-x)); return s * (1.0f + x * (1.0f - s)); }
    case 2: return x > 0.0f ? 1.0f : 0.0f;
    case 3: return x > 0.0f ? 1.0f : 0.1f;
    case 4: { float t = tanhf(x); return 1.0f - t * t; }
    case 5: { float s = 1.0f / (1.0f + __expf(-x)); return s * (1.0f - s); }
    default: return x < 10.0f ? __expf(x) : 0.0f;
    }
}

__global__ void k_act_fwd(const float* x, float* y, size_t n, int op) {
    GRID_LOOP(i, n) y[i] = act_f(op, x[i]);
}
__global__ void k_act_bwd(const float* x, const float* d, float* dx, size_t n, int op) {
    GRID_LOOP(i, n) dx[i] += d[i] * act_g(op, x[i]);
}

// ---------------------------------------------------------------------------
// Elementwise
// ---------------------------------------------------------------------------

__global__ void k_add(const float* a, const float* b, float* y, size_t n) { GRID_LOOP(i, n) y[i] = a[i] + b[i]; }
__global__ void k_mul(const float* a, const float* b, float* y, size_t n) { GRID_LOOP(i, n) y[i] = a[i] * b[i]; }
__global__ void k_scale(const float* x, float* y, float s, size_t n) { GRID_LOOP(i, n) y[i] = x[i] * s; }
__global__ void k_axpy(const float* x, float* y, float s, size_t n) { GRID_LOOP(i, n) y[i] += s * x[i]; }
// y += x * other (the product rule's half)
__global__ void k_mul_acc(const float* x, const float* other, float* y, size_t n) { GRID_LOOP(i, n) y[i] += x[i] * other[i]; }
__global__ void k_fill(float* y, float v, size_t n) { GRID_LOOP(i, n) y[i] = v; }
// y += s * x[0] (scalar broadcast; used by scalar loss sums)
__global__ void k_axpy_scalar(const float* x, float* y, float s) { if (threadIdx.x == 0 && blockIdx.x == 0) y[0] += s * x[0]; }
__global__ void k_log_eps(const float* x, float* y, float eps, size_t n) { GRID_LOOP(i, n) y[i] = logf(x[i] + eps); }
__global__ void k_log_eps_bwd(const float* x, const float* d, float* dx, float eps, size_t n) { GRID_LOOP(i, n) dx[i] += d[i] / (x[i] + eps); }

// Row-broadcast: y[r,c] = x[r,c] (+|*) v[c]
__global__ void k_add_row(const float* x, const float* v, float* y, size_t rows, size_t cols) {
    GRID_LOOP(i, rows * cols) y[i] = x[i] + v[i % cols];
}
__global__ void k_mul_row(const float* x, const float* v, float* y, size_t rows, size_t cols) {
    GRID_LOOP(i, rows * cols) y[i] = x[i] * v[i % cols];
}
__global__ void k_mul_row_bwd_x(const float* d, const float* v, float* dx, size_t rows, size_t cols) {
    GRID_LOOP(i, rows * cols) dx[i] += d[i] * v[i % cols];
}
__global__ void k_mul_col(const float* x, const float* s, float* y, size_t rows, size_t cols) {
    GRID_LOOP(i, rows * cols) y[i] = x[i] * s[i / cols];
}
__global__ void k_mul_col_bwd_x(const float* d, const float* s, float* dx, size_t rows, size_t cols) {
    GRID_LOOP(i, rows * cols) dx[i] += d[i] * s[i / cols];
}

// dv[c] += sum_r d[r,c] * (x ? x[r,c] : 1). grid.x covers columns in 32s,
// block (32, 8): each thread strides rows, a shared reduction per column.
__global__ void k_col_dot_acc(const float* d, const float* x, float* dv, size_t rows, size_t cols) {
    __shared__ float sh[8][33];
    size_t c = blockIdx.x * 32 + threadIdx.x;
    float acc = 0.0f;
    if (c < cols) {
        for (size_t r = blockIdx.y * 8 + threadIdx.y; r < rows; r += 8 * gridDim.y) {
            float v = d[r * cols + c];
            acc += x ? v * x[r * cols + c] : v;
        }
    }
    sh[threadIdx.y][threadIdx.x] = acc;
    __syncthreads();
    if (threadIdx.y == 0 && c < cols) {
        float s = 0.0f;
        for (int k = 0; k < 8; k++) s += sh[k][threadIdx.x];
        atomicAdd(&dv[c], s);
    }
}

// ds[r] += sum_c d[r,c] * x[r,c]; one warp per row.
__global__ void k_row_dot_acc(const float* d, const float* x, float* ds, size_t rows, size_t cols) {
    size_t r = blockIdx.x * (blockDim.x / 32) + threadIdx.x / 32;
    int lane = threadIdx.x & 31;
    if (r >= rows) return;
    float acc = 0.0f;
    for (size_t c = lane; c < cols; c += 32) acc += d[r * cols + c] * x[r * cols + c];
    for (int o = 16; o > 0; o >>= 1) acc += __shfl_down_sync(0xffffffff, acc, o);
    if (lane == 0) ds[r] += acc;
}

// Copy a block of columns: dst[r, dst_off + c] (=|+=) src[r, src_off + c].
__global__ void k_copy_cols(const float* src, size_t src_cols, size_t src_off, float* dst, size_t dst_cols, size_t dst_off, size_t rows, size_t width, int acc) {
    GRID_LOOP(i, rows * width) {
        size_t r = i / width, c = i % width;
        float v = src[r * src_cols + src_off + c];
        float* o = &dst[r * dst_cols + dst_off + c];
        if (acc) *o += v; else *o = v;
    }
}

__global__ void k_gather_rows(const float* x, const uint32_t* idx, float* y, size_t rows_out, size_t cols) {
    GRID_LOOP(i, rows_out * cols) {
        size_t r = i / cols, c = i % cols;
        y[i] = x[(size_t)idx[r] * cols + c];
    }
}
__global__ void k_scatter_rows_acc(const float* d, const uint32_t* idx, float* dx, size_t rows_out, size_t cols) {
    GRID_LOOP(i, rows_out * cols) {
        size_t r = i / cols, c = i % cols;
        atomicAdd(&dx[(size_t)idx[r] * cols + c], d[i]);
    }
}

// ---------------------------------------------------------------------------
// Convolutions over time within segments of `seg` rows (same padding)
// ---------------------------------------------------------------------------

// col[r, j*C + c] = x[r + j*dil - pad, c] inside r's segment, else 0.
__global__ void k_im2col(const float* x, float* col, size_t rows, size_t C, int k, int dil, size_t seg) {
    int pad = (k - 1) * dil / 2;
    GRID_LOOP(i, rows * (size_t)k * C) {
        size_t r = i / ((size_t)k * C);
        size_t rem = i % ((size_t)k * C);
        int j = (int)(rem / C);
        size_t c = rem % C;
        size_t base = (r / seg) * seg;
        long src = (long)(r - base) + (long)j * dil - pad;
        col[i] = (src >= 0 && (size_t)src < seg && base + src < rows) ? x[(base + src) * C + c] : 0.0f;
    }
}
// dx[s, c] += sum_j dcol[s - j*dil + pad, j*C + c] (inside the segment)
__global__ void k_col2im_acc(const float* dcol, float* dx, size_t rows, size_t C, int k, int dil, size_t seg) {
    int pad = (k - 1) * dil / 2;
    GRID_LOOP(i, rows * C) {
        size_t s = i / C, c = i % C;
        size_t base = (s / seg) * seg;
        float acc = 0.0f;
        for (int j = 0; j < k; j++) {
            long r = (long)(s - base) - (long)j * dil + pad;
            if (r >= 0 && (size_t)r < seg && base + r < rows) acc += dcol[(base + r) * (size_t)k * C + (size_t)j * C + c];
        }
        dx[i] += acc;
    }
}

// Depthwise: y[r,c] = sum_j x[r + j - pad, c] * w[j, c]
__global__ void k_dwconv(const float* x, const float* w, float* y, size_t rows, size_t C, int k, size_t seg) {
    int pad = (k - 1) / 2;
    GRID_LOOP(i, rows * C) {
        size_t r = i / C, c = i % C;
        size_t base = (r / seg) * seg;
        float acc = 0.0f;
        for (int j = 0; j < k; j++) {
            long s = (long)(r - base) + j - pad;
            if (s >= 0 && (size_t)s < seg && base + s < rows) acc += x[(base + s) * C + c] * w[(size_t)j * C + c];
        }
        y[i] = acc;
    }
}
__global__ void k_dwconv_bwd_x(const float* d, const float* w, float* dx, size_t rows, size_t C, int k, size_t seg) {
    int pad = (k - 1) / 2;
    GRID_LOOP(i, rows * C) {
        size_t s = i / C, c = i % C;
        size_t base = (s / seg) * seg;
        float acc = 0.0f;
        for (int j = 0; j < k; j++) {
            long r = (long)(s - base) - j + pad;
            if (r >= 0 && (size_t)r < seg && base + r < rows) acc += d[(base + r) * C + c] * w[(size_t)j * C + c];
        }
        dx[i] += acc;
    }
}
// dw[j, c] += sum_r d[r, c] * x[r + j - pad, c]; grid (ceil(C/32), k, ychunks), block (32, 8)
__global__ void k_dwconv_bwd_w(const float* d, const float* x, float* dw, size_t rows, size_t C, int k, size_t seg) {
    __shared__ float sh[8][33];
    int pad = (k - 1) / 2;
    size_t c = blockIdx.x * 32 + threadIdx.x;
    int j = blockIdx.y;
    float acc = 0.0f;
    if (c < C) {
        for (size_t r = blockIdx.z * 8 + threadIdx.y; r < rows; r += 8 * gridDim.z) {
            size_t base = (r / seg) * seg;
            long s = (long)(r - base) + j - pad;
            if (s >= 0 && (size_t)s < seg && base + s < rows) acc += d[r * C + c] * x[(base + s) * C + c];
        }
    }
    sh[threadIdx.y][threadIdx.x] = acc;
    __syncthreads();
    if (threadIdx.y == 0 && c < C) {
        float s = 0.0f;
        for (int q = 0; q < 8; q++) s += sh[q][threadIdx.x];
        atomicAdd(&dw[(size_t)j * C + c], s);
    }
}

// ---------------------------------------------------------------------------
// LayerNorm (no affine; the gain and bias are mul_row / add_row)
// ---------------------------------------------------------------------------

__device__ float block_sum(float v, float* sh) {
    for (int o = 16; o > 0; o >>= 1) v += __shfl_down_sync(0xffffffff, v, o);
    int lane = threadIdx.x & 31, w = threadIdx.x >> 5;
    if (lane == 0) sh[w] = v;
    __syncthreads();
    int nw = blockDim.x >> 5;
    v = threadIdx.x < nw ? sh[threadIdx.x] : 0.0f;
    if (w == 0) for (int o = 16; o > 0; o >>= 1) v += __shfl_down_sync(0xffffffff, v, o);
    if (threadIdx.x == 0) sh[0] = v;
    __syncthreads();
    float r = sh[0];
    __syncthreads();
    return r;
}

// One block per row.
__global__ void k_layer_norm(const float* x, float* xhat, float* rstd, size_t cols) {
    __shared__ float sh[32];
    size_t r = blockIdx.x;
    const float* xr = x + r * cols;
    float s = 0.0f;
    for (size_t c = threadIdx.x; c < cols; c += blockDim.x) s += xr[c];
    float mean = block_sum(s, sh) / cols;
    float v = 0.0f;
    for (size_t c = threadIdx.x; c < cols; c += blockDim.x) { float e = xr[c] - mean; v += e * e; }
    float var = block_sum(v, sh) / cols;
    float rs = rsqrtf(var + 1e-5f);
    for (size_t c = threadIdx.x; c < cols; c += blockDim.x) xhat[r * cols + c] = (xr[c] - mean) * rs;
    if (threadIdx.x == 0) rstd[r] = rs;
}
__global__ void k_layer_norm_bwd(const float* d, const float* xhat, const float* rstd, float* dx, size_t cols) {
    __shared__ float sh[32];
    size_t r = blockIdx.x;
    const float* dr = d + r * cols;
    const float* xr = xhat + r * cols;
    float a = 0.0f, b = 0.0f;
    for (size_t c = threadIdx.x; c < cols; c += blockDim.x) { a += dr[c]; b += dr[c] * xr[c]; }
    float m1 = block_sum(a, sh) / cols;
    float m2 = block_sum(b, sh) / cols;
    float rs = rstd[r];
    for (size_t c = threadIdx.x; c < cols; c += blockDim.x) dx[r * cols + c] += rs * (dr[c] - m1 - xr[c] * m2);
}

// ---------------------------------------------------------------------------
// RoPE: per head, pairs (i, i + dh/2); position = row index in its segment.
// sign = +1 forward, -1 the inverse rotation (the backward).
// ---------------------------------------------------------------------------

__global__ void k_rope(const float* x, float* y, size_t rows, size_t C, int heads, size_t seg, float sign, int acc) {
    int dh = (int)(C / heads), half = dh / 2;
    GRID_LOOP(i, rows * (size_t)heads * half) {
        size_t r = i / ((size_t)heads * half);
        size_t rem = i % ((size_t)heads * half);
        int h = (int)(rem / half), q = (int)(rem % half);
        float pos = (float)(r % seg);
        float f = powf(10000.0f, -(float)q / half);
        float sn, cs;
        sincosf(pos * f, &sn, &cs);
        size_t a = r * C + (size_t)h * dh + q, b = a + half;
        float xa = x[a], xb = x[b];
        float ya = xa * cs - sign * xb * sn, yb = sign * xa * sn + xb * cs;
        if (acc) { y[a] += ya; y[b] += yb; } else { y[a] = ya; y[b] = yb; }
    }
}

// ---------------------------------------------------------------------------
// Attention helpers: [B*T, H*dh] <-> [B, H, T, dh], masked softmax
// ---------------------------------------------------------------------------

__global__ void k_split_heads(const float* x, float* y, size_t B, size_t T, int H, int dh) {
    GRID_LOOP(i, B * T * (size_t)H * dh) {
        size_t e = i % dh, t = (i / dh) % T, h = (i / ((size_t)dh * T)) % H, b = i / ((size_t)dh * T * H);
        y[i] = x[(b * T + t) * (size_t)H * dh + h * dh + e];
    }
}
__global__ void k_merge_heads(const float* x, float* y, size_t B, size_t T, int H, int dh, int acc) {
    GRID_LOOP(i, B * T * (size_t)H * dh) {
        size_t e = i % dh, t = (i / dh) % T, h = (i / ((size_t)dh * T)) % H, b = i / ((size_t)dh * T * H);
        float v = x[i];
        float* o = &y[(b * T + t) * (size_t)H * dh + h * dh + e];
        if (acc) *o += v; else *o = v;
    }
}

// In place: rows of s [B*H*T, S] scaled, keys >= key_len[b] masked, softmax.
// One block per row.
__global__ void k_softmax_rows(float* s, size_t T, size_t S, int H, const uint32_t* key_len, float scale) {
    __shared__ float sh[32];
    size_t row = blockIdx.x;
    size_t b = row / ((size_t)H * T);
    size_t valid = key_len ? key_len[b] : S;
    float* p = s + row * S;
    float mx = -1e30f;
    for (size_t j = threadIdx.x; j < S; j += blockDim.x) if (j < valid) mx = fmaxf(mx, p[j] * scale);
    // block max
    for (int o = 16; o > 0; o >>= 1) mx = fmaxf(mx, __shfl_down_sync(0xffffffff, mx, o));
    int lane = threadIdx.x & 31, w = threadIdx.x >> 5;
    if (lane == 0) sh[w] = mx;
    __syncthreads();
    if (threadIdx.x == 0) { float m = -1e30f; for (int k = 0; k < (int)(blockDim.x >> 5); k++) m = fmaxf(m, sh[k]); sh[0] = m; }
    __syncthreads();
    mx = sh[0];
    __syncthreads();
    float sum = 0.0f;
    for (size_t j = threadIdx.x; j < S; j += blockDim.x) {
        float e = j < valid ? __expf(p[j] * scale - mx) : 0.0f;
        p[j] = e;
        sum += e;
    }
    sum = block_sum(sum, sh);
    float inv = 1.0f / fmaxf(sum, 1e-30f);
    for (size_t j = threadIdx.x; j < S; j += blockDim.x) p[j] *= inv;
}
// dp -> ds in place: ds = p * (dp - sum(dp * p)) * scale
__global__ void k_softmax_bwd_rows(const float* p, float* dp, size_t S, float scale) {
    __shared__ float sh[32];
    size_t row = blockIdx.x;
    const float* pr = p + row * S;
    float* dr = dp + row * S;
    float dot = 0.0f;
    for (size_t j = threadIdx.x; j < S; j += blockDim.x) dot += pr[j] * dr[j];
    dot = block_sum(dot, sh);
    for (size_t j = threadIdx.x; j < S; j += blockDim.x) dr[j] = pr[j] * (dr[j] - dot) * scale;
}

// ---------------------------------------------------------------------------
// Losses. Row weights w[r] (the mask), columns >= col_lim[item] ignored
// (col_lim per item = rows/seg items; null = all). Writes the per-element
// gradient coefficient into g (later scaled by the upstream scalar) and
// accumulates the loss sum into out[0].
// ---------------------------------------------------------------------------

__global__ void k_loss(const float* x, const float* t, const float* w, const uint32_t* col_lim, size_t seg, float* g, float* out, size_t rows, size_t cols, float inv_denom, int sq) {
    __shared__ float sh[32];
    float acc = 0.0f;
    for (size_t i = blockIdx.x * (size_t)blockDim.x + threadIdx.x; i < rows * cols; i += (size_t)blockDim.x * gridDim.x) {
        size_t r = i / cols, c = i % cols;
        float wr = w ? w[r] : 1.0f;
        if (col_lim && c >= col_lim[r / seg]) wr = 0.0f;
        float e = x[i] - t[i];
        if (sq) { acc += wr * e * e; g[i] = 2.0f * wr * e * inv_denom; }
        else { acc += wr * fabsf(e); g[i] = wr * (e > 0.0f ? 1.0f : (e < 0.0f ? -1.0f : 0.0f)) * inv_denom; }
    }
    acc = block_sum(acc, sh);
    if (threadIdx.x == 0) atomicAdd(out, acc * inv_denom);
}
// dx += g * d[0]
__global__ void k_scaled_acc(const float* g, const float* d, float* dx, size_t n) {
    float s = d[0];
    GRID_LOOP(i, n) dx[i] += g[i] * s;
}
__global__ void k_bce(const float* x, const float* t, float* g, float* out, size_t n) {
    __shared__ float sh[32];
    float acc = 0.0f;
    float inv = 1.0f / n;
    for (size_t i = blockIdx.x * (size_t)blockDim.x + threadIdx.x; i < n; i += (size_t)blockDim.x * gridDim.x) {
        float z = x[i], y = t[i];
        acc += fmaxf(z, 0.0f) - z * y + log1pf(__expf(-fabsf(z)));
        g[i] = (1.0f / (1.0f + __expf(-z)) - y) * inv;
    }
    acc = block_sum(acc, sh);
    if (threadIdx.x == 0) atomicAdd(out, acc * inv);
}

// ---------------------------------------------------------------------------
// The vocoder's source filter (see nn.rs hn_filter)
// ---------------------------------------------------------------------------

__global__ void k_hn_filter(const float* gh, const float* gn, const float* pa, const float* pb,
                            const float* hr, const float* hi, const float* nr, const float* ni,
                            float* y, size_t rows, size_t F) {
    GRID_LOOP(i, rows * F) {
        size_t r = i / F, q = i % F;
        float ah = __expf(fminf(gh[i], 10.0f)), an = __expf(fminf(gn[i], 10.0f));
        float a = pa[i], b = pb[i];
        float m = sqrtf(a * a + b * b + 1e-6f);
        float cs = a / m, sn = b / m;
        float xr = hr[i] * cs - hi[i] * sn, xi = hr[i] * sn + hi[i] * cs;
        y[r * 2 * F + q] = ah * xr + an * nr[i];
        y[r * 2 * F + F + q] = ah * xi + an * ni[i];
    }
}
__global__ void k_hn_filter_bwd(const float* d, const float* gh, const float* gn, const float* pa, const float* pb,
                                const float* hr, const float* hi, const float* nr, const float* ni,
                                float* dgh, float* dgn, float* dpa, float* dpb, size_t rows, size_t F) {
    GRID_LOOP(i, rows * F) {
        size_t r = i / F, q = i % F;
        float dyr = d[r * 2 * F + q], dyi = d[r * 2 * F + F + q];
        float ah = __expf(fminf(gh[i], 10.0f)), an = __expf(fminf(gn[i], 10.0f));
        float a = pa[i], b = pb[i];
        float m2 = a * a + b * b + 1e-6f, m = sqrtf(m2);
        float cs = a / m, sn = b / m;
        float xr = hr[i] * cs - hi[i] * sn, xi = hr[i] * sn + hi[i] * cs;
        if (dgh) dgh[i] += (dyr * xr + dyi * xi) * ah * (gh[i] < 10.0f ? 1.0f : 0.0f);
        if (dgn) dgn[i] += (dyr * nr[i] + dyi * ni[i]) * an * (gn[i] < 10.0f ? 1.0f : 0.0f);
        float dcs = ah * (dyr * hr[i] + dyi * hi[i]);
        float dsn = ah * (-dyr * hi[i] + dyi * hr[i]);
        float m3 = m2 * m;
        if (dpa) dpa[i] += dcs * (b * b + 1e-6f) / m3 - dsn * a * b / m3;
        if (dpb) dpb[i] += dsn * (a * a + 1e-6f) / m3 - dcs * a * b / m3;
    }
}

// ---------------------------------------------------------------------------
// FFT: one block per transform, in shared memory, n a power of two <= 4096.
// Forward e^{-i}, inverse e^{+i} unscaled.
// ---------------------------------------------------------------------------

__global__ void k_fft(float* re, float* im, int n, int log2n, int inverse) {
    extern __shared__ float s[];
    float* sr = s;
    float* si = s + n;
    size_t base = (size_t)blockIdx.x * n;
    for (int i = threadIdx.x; i < n; i += blockDim.x) {
        int j = __brev(i) >> (32 - log2n);
        sr[j] = re[base + i];
        si[j] = im[base + i];
    }
    __syncthreads();
    float sign = inverse ? 1.0f : -1.0f;
    for (int len = 2; len <= n; len <<= 1) {
        int half = len >> 1;
        for (int t = threadIdx.x; t < n / 2; t += blockDim.x) {
            int grp = t / half, k = t % half;
            int a = grp * len + k, b = a + half;
            float ang = sign * 2.0f * 3.14159265358979f * k / len;
            float wr, wi;
            sincosf(ang, &wi, &wr);
            float tr = sr[b] * wr - si[b] * wi;
            float ti = sr[b] * wi + si[b] * wr;
            sr[b] = sr[a] - tr; si[b] = si[a] - ti;
            sr[a] += tr; si[a] += ti;
        }
        __syncthreads();
    }
    for (int i = threadIdx.x; i < n; i += blockDim.x) {
        re[base + i] = sr[i];
        im[base + i] = si[i];
    }
}

// Frames of B waveforms (each `len` samples) into [B*frames, n] with the
// window (length n, zero-padded) and an optional gain per sample of an item
// (`len` values, shared by all items), im = 0.
__global__ void k_frame(const float* x, const float* gain, const float* win, float* re, float* im, size_t B, size_t len, size_t frames, int n, int hop) {
    GRID_LOOP(i, B * frames * (size_t)n) {
        size_t k = i % n, f = (i / n) % frames, b = i / ((size_t)n * frames);
        long src = (long)(f * hop) - n / 2 + (long)k;
        float v = 0.0f;
        if (src >= 0 && (size_t)src < len) {
            size_t si = b * len + src;
            v = x[si] * win[k] * (gain ? gain[src] : 1.0f);
        }
        re[i] = v;
        im[i] = 0.0f;
    }
}
// Full-spectrum [rows, n] -> half [rows, bins] (re | separate im)
__global__ void k_half(const float* fr, const float* fi, float* hr, float* hi, size_t rows, int n, float scale) {
    int bins = n / 2 + 1;
    GRID_LOOP(i, rows * (size_t)bins) {
        size_t r = i / bins, q = i % bins;
        hr[i] = fr[r * n + q] * scale;
        hi[i] = fi[r * n + q] * scale;
    }
}
// Full -> half with per-bin weights (c_edge at DC/Nyquist, whose imaginary
// part is zeroed; c_mid elsewhere): the iSTFT's adjoint.
__global__ void k_half_c(const float* fr, const float* fi, float* hr, float* hi, size_t rows, int n, float c_edge, float c_mid) {
    int bins = n / 2 + 1;
    GRID_LOOP(i, rows * (size_t)bins) {
        size_t r = i / bins; int q = (int)(i % bins);
        bool edge = q == 0 || q == n / 2;
        float c = edge ? c_edge : c_mid;
        hr[i] = fr[r * n + q] * c;
        hi[i] = edge ? 0.0f : fi[r * n + q] * c;
    }
}
// Half spectrum (with per-bin weights c_edge at DC/Nyquist and c_mid elsewhere,
// imaginary part dropped at the edges) -> Hermitian full spectrum.
__global__ void k_hermitian(const float* hr, const float* hi, float* fr, float* fi, size_t rows, int n, float c_edge, float c_mid) {
    int bins = n / 2 + 1;
    GRID_LOOP(i, rows * (size_t)n) {
        size_t r = i / n;
        int k = (int)(i % n);
        int q = k < bins ? k : n - k;
        float c = (q == 0 || q == n / 2) ? c_edge : c_mid;
        float vr = hr[r * bins + q] * c;
        float vi = (q == 0 || q == n / 2) ? 0.0f : hi[r * bins + q] * c;
        fr[i] = vr;
        fi[i] = k < bins ? vi : -vi;
    }
}
// Overlap-add of windowed frames [B*frames, n] (real part) into B waveforms:
// y[b, s] (=|+=) norm[s] * sum_f win[k] * fr[(b, f), k] * scale
__global__ void k_overlap_add(const float* fr, const float* win, const float* norm, float* y, size_t B, size_t len, size_t frames, int n, int hop, float scale, int acc) {
    GRID_LOOP(i, B * len) {
        size_t b = i / len, s = i % len;
        long c = (long)s + n / 2;
        long f_hi = c / hop;
        long f_lo = (c - n + 1 + hop - 1) / hop;
        if (f_lo < 0) f_lo = 0;
        if (f_hi > (long)frames - 1) f_hi = (long)frames - 1;
        float v = 0.0f;
        for (long f = f_lo; f <= f_hi; f++) {
            long k = c - f * hop;
            if (k >= 0 && k < n) v += win[k] * fr[(b * frames + f) * n + k];
        }
        v *= scale * (norm ? norm[s] : 1.0f);
        if (acc) y[i] += v; else y[i] = v;
    }
}
__global__ void k_mag(const float* re, const float* im, float* m, size_t n) {
    GRID_LOOP(i, n) m[i] = sqrtf(re[i] * re[i] + im[i] * im[i] + 1e-9f);
}
__global__ void k_mag_bwd(const float* d, const float* re, const float* im, const float* m, float* dre, float* dim, size_t n) {
    GRID_LOOP(i, n) { float s = d[i] / m[i]; dre[i] = s * re[i]; dim[i] = s * im[i]; }
}
// Deinterleave [rows, 2F] (re | im) <-> separate [rows, F] planes.
__global__ void k_split_ri(const float* y, float* re, float* im, size_t rows, size_t F) {
    GRID_LOOP(i, rows * F) { size_t r = i / F, q = i % F; re[i] = y[r * 2 * F + q]; im[i] = y[r * 2 * F + F + q]; }
}
__global__ void k_join_ri_acc(const float* re, const float* im, float* y, size_t rows, size_t F) {
    GRID_LOOP(i, rows * F) { size_t r = i / F, q = i % F; y[r * 2 * F + q] += re[i]; y[r * 2 * F + F + q] += im[i]; }
}

// ---------------------------------------------------------------------------
// Optimiser
// ---------------------------------------------------------------------------

__global__ void k_sumsq(const float* g, size_t n, float* out) {
    __shared__ float sh[32];
    float acc = 0.0f;
    for (size_t i = blockIdx.x * (size_t)blockDim.x + threadIdx.x; i < n; i += (size_t)blockDim.x * gridDim.x) acc += g[i] * g[i];
    acc = block_sum(acc, sh);
    if (threadIdx.x == 0) atomicAdd(out, acc);
}
// AdamW with the gradient scaled by gscale_ptr[0] (clipping), decoupled decay.
__global__ void k_adamw(float* p, const float* g, float* m, float* v, size_t n, float lr, float b1, float b2, float eps, float wd, float bc1, float bc2, const float* gscale_ptr) {
    float gs = gscale_ptr ? gscale_ptr[0] : 1.0f;
    GRID_LOOP(i, n) {
        float gi = g[i] * gs;
        float mi = b1 * m[i] + (1.0f - b1) * gi;
        float vi = b2 * v[i] + (1.0f - b2) * gi * gi;
        m[i] = mi; v[i] = vi;
        float upd = (mi / bc1) / (sqrtf(vi / bc2) + eps);
        p[i] = p[i] * (1.0f - lr * wd) - lr * upd;
    }
}
// clip: scale = min(1, max_norm / sqrt(sumsq))
__global__ void k_clip_scale(const float* sumsq, float max_norm, float* scale) {
    if (threadIdx.x == 0 && blockIdx.x == 0) { float nrm = sqrtf(sumsq[0]); scale[0] = nrm > max_norm ? max_norm / nrm : 1.0f; }
}
__global__ void k_ema(float* e, const float* p, float decay, size_t n) { GRID_LOOP(i, n) e[i] = decay * e[i] + (1.0f - decay) * p[i]; }

// Counter-based normal noise (for the flow refiner's training targets).
__device__ __forceinline__ uint64_t mix64(uint64_t z) {
    z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9ull;
    z = (z ^ (z >> 27)) * 0x94D049BB133111EBull;
    return z ^ (z >> 31);
}
__global__ void k_randn(float* y, size_t n, uint64_t seed) {
    GRID_LOOP(i, n) {
        uint64_t a = mix64(seed + 2 * i * 0x9E3779B97F4A7C15ull), b = mix64(seed + (2 * i + 1) * 0x9E3779B97F4A7C15ull);
        float u1 = ((a >> 40) + 1) * (1.0f / 16777217.0f);
        float u2 = (b >> 40) * (1.0f / 16777216.0f);
        y[i] = sqrtf(-2.0f * logf(u1)) * cospif(2.0f * u2);
    }
}

// The harmonic source from per-sample controls (see dsp.rs harmonic_sample).
__global__ void k_harmonic(const float* ph, const float* hz, const float* amp, float* y, size_t n, float top, float level) {
    GRID_LOOP(i, n) {
        float f = hz[i], a = amp[i];
        float acc = 0.0f;
        if (a > 0.0f && f > 0.0f) {
            float taper_lo = top * 0.8f;
            int count = (int)(top / f);
            float c2 = 2.0f * cosf(ph[i]);
            float sp = 0.0f, s = sinf(ph[i]);
            for (int k = 1; k <= count; k++) {
                float fk = k * f;
                float g = fk > taper_lo ? (top - fk) / (top - taper_lo) : 1.0f;
                acc += s * g;
                float nx = c2 * s - sp;
                sp = s;
                s = nx;
            }
        }
        y[i] = acc * level * a;
    }
}

// ---------------------------------------------------------------------------
// Monotonic alignment search: logp [B, N, T] (token x frame log-likelihood),
// n_tok[b], n_frame[b]; writes each token's frame count dur[b, n] (u32).
// One block per item; threads over tokens; the DP table is written over logp.
// ---------------------------------------------------------------------------

__global__ void k_mas(float* logp, const uint32_t* n_tok, const uint32_t* n_frame, uint32_t* dur, int N, int T) {
    int b = blockIdx.x;
    int nt = n_tok[b], nf = n_frame[b];
    float* q = logp + (size_t)b * N * T;
    const float NEG = -1e30f;
    // Column 0.
    for (int i = threadIdx.x; i < nt; i += blockDim.x) q[(size_t)i * T] = i == 0 ? q[0] : NEG;
    __syncthreads();
    for (int j = 1; j < nf; j++) {
        // Update rows from high to low is unnecessary: read column j-1 only.
        for (int i = threadIdx.x; i < nt; i += blockDim.x) {
            float stay = q[(size_t)i * T + j - 1];
            float move = i > 0 ? q[(size_t)(i - 1) * T + j - 1] : NEG;
            float best = fmaxf(stay, move);
            // A token cannot start before frame i, nor leave too few frames.
            if (i > j || nt - 1 - i > nf - 1 - j) best = NEG;
            q[(size_t)i * T + j] = best + q[(size_t)i * T + j];
        }
        __syncthreads();
    }
    if (threadIdx.x == 0) {
        for (int i = 0; i < N; i++) dur[(size_t)b * N + i] = 0;
        int i = nt - 1;
        for (int j = nf - 1; j >= 0; j--) {
            dur[(size_t)b * N + i] += 1;
            if (i > 0 && j > 0 && (i == j || q[(size_t)(i - 1) * T + j - 1] > q[(size_t)i * T + j - 1])) i--;
        }
    }
}

// logp[b, i, j] = dot[b, i, j] - 0.5 |mu_bi|^2 - 0.5 |mel_bj|^2 (in place over dot)
__global__ void k_mas_logp(float* dot, const float* mu, const float* mel, size_t B, int N, int T, int D) {
    GRID_LOOP(x, B * (size_t)N * T) {
        size_t b = x / ((size_t)N * T);
        int i = (int)((x / T) % N), j = (int)(x % T);
        const float* m = mu + (b * N + i) * D;
        const float* e = mel + (b * T + j) * D;
        float a = 0.0f, c = 0.0f;
        for (int k = 0; k < D; k++) { a += m[k] * m[k]; c += e[k] * e[k]; }
        dot[x] = dot[x] - 0.5f * a - 0.5f * c;
    }
}
// Durations -> the encoder row of every frame (padded frames: the item's last row),
// and ln(1 + frames) per token as f32.
__global__ void k_dur_to_idx(const uint32_t* dur, uint32_t* idx, float* logdur, size_t B, int N, int T) {
    size_t b = blockIdx.x * (size_t)blockDim.x + threadIdx.x;
    if (b >= B) return;
    int f = 0;
    for (int i = 0; i < N; i++) {
        uint32_t d = dur[b * N + i];
        logdur[b * N + i] = log1pf((float)d);
        for (uint32_t k = 0; k < d && f < T; k++) idx[b * T + f++] = (uint32_t)(b * N + i);
    }
    for (; f < T; f++) idx[b * T + f] = (uint32_t)(b * N + N - 1);
}

// ---------------------------------------------------------------------------
// Launch wrappers
// ---------------------------------------------------------------------------

#define L1(k, n, ...) do { k<<<mkt_blocks(n), MKT_THREADS, 0, st>>>(__VA_ARGS__); return cudaGetLastError(); } while (0)

extern "C" {
cudaError_t mkt_act_fwd(const float* x, float* y, size_t n, int op, cudaStream_t st) { L1(k_act_fwd, n, x, y, n, op); }
cudaError_t mkt_act_bwd(const float* x, const float* d, float* dx, size_t n, int op, cudaStream_t st) { L1(k_act_bwd, n, x, d, dx, n, op); }
cudaError_t mkt_add(const float* a, const float* b, float* y, size_t n, cudaStream_t st) { L1(k_add, n, a, b, y, n); }
cudaError_t mkt_mul(const float* a, const float* b, float* y, size_t n, cudaStream_t st) { L1(k_mul, n, a, b, y, n); }
cudaError_t mkt_scale(const float* x, float* y, float s, size_t n, cudaStream_t st) { L1(k_scale, n, x, y, s, n); }
cudaError_t mkt_axpy(const float* x, float* y, float s, size_t n, cudaStream_t st) { L1(k_axpy, n, x, y, s, n); }
cudaError_t mkt_mul_acc(const float* x, const float* o, float* y, size_t n, cudaStream_t st) { L1(k_mul_acc, n, x, o, y, n); }
cudaError_t mkt_fill(float* y, float v, size_t n, cudaStream_t st) { L1(k_fill, n, y, v, n); }
cudaError_t mkt_axpy_scalar(const float* x, float* y, float s, cudaStream_t st) { k_axpy_scalar<<<1, 32, 0, st>>>(x, y, s); return cudaGetLastError(); }
cudaError_t mkt_log_eps(const float* x, float* y, float eps, size_t n, cudaStream_t st) { L1(k_log_eps, n, x, y, eps, n); }
cudaError_t mkt_log_eps_bwd(const float* x, const float* d, float* dx, float eps, size_t n, cudaStream_t st) { L1(k_log_eps_bwd, n, x, d, dx, eps, n); }
cudaError_t mkt_add_row(const float* x, const float* v, float* y, size_t rows, size_t cols, cudaStream_t st) { L1(k_add_row, rows * cols, x, v, y, rows, cols); }
cudaError_t mkt_mul_row(const float* x, const float* v, float* y, size_t rows, size_t cols, cudaStream_t st) { L1(k_mul_row, rows * cols, x, v, y, rows, cols); }
cudaError_t mkt_mul_row_bwd_x(const float* d, const float* v, float* dx, size_t rows, size_t cols, cudaStream_t st) { L1(k_mul_row_bwd_x, rows * cols, d, v, dx, rows, cols); }
cudaError_t mkt_mul_col(const float* x, const float* s, float* y, size_t rows, size_t cols, cudaStream_t st) { L1(k_mul_col, rows * cols, x, s, y, rows, cols); }
cudaError_t mkt_mul_col_bwd_x(const float* d, const float* s, float* dx, size_t rows, size_t cols, cudaStream_t st) { L1(k_mul_col_bwd_x, rows * cols, d, s, dx, rows, cols); }
cudaError_t mkt_col_dot_acc(const float* d, const float* x, float* dv, size_t rows, size_t cols, cudaStream_t st) {
    unsigned gy = (unsigned)((rows + 255) / 256); if (gy > 128) gy = 128; if (gy == 0) gy = 1;
    dim3 grid((unsigned)((cols + 31) / 32), gy);
    k_col_dot_acc<<<grid, dim3(32, 8), 0, st>>>(d, x, dv, rows, cols);
    return cudaGetLastError();
}
cudaError_t mkt_row_dot_acc(const float* d, const float* x, float* ds, size_t rows, size_t cols, cudaStream_t st) {
    k_row_dot_acc<<<(unsigned)((rows + 7) / 8), 256, 0, st>>>(d, x, ds, rows, cols);
    return cudaGetLastError();
}
cudaError_t mkt_copy_cols(const float* src, size_t src_cols, size_t src_off, float* dst, size_t dst_cols, size_t dst_off, size_t rows, size_t width, int acc, cudaStream_t st) {
    L1(k_copy_cols, rows * width, src, src_cols, src_off, dst, dst_cols, dst_off, rows, width, acc);
}
cudaError_t mkt_gather_rows(const float* x, const uint32_t* idx, float* y, size_t rows_out, size_t cols, cudaStream_t st) { L1(k_gather_rows, rows_out * cols, x, idx, y, rows_out, cols); }
cudaError_t mkt_scatter_rows_acc(const float* d, const uint32_t* idx, float* dx, size_t rows_out, size_t cols, cudaStream_t st) { L1(k_scatter_rows_acc, rows_out * cols, d, idx, dx, rows_out, cols); }
cudaError_t mkt_im2col(const float* x, float* col, size_t rows, size_t C, int k, int dil, size_t seg, cudaStream_t st) { L1(k_im2col, rows * (size_t)k * C, x, col, rows, C, k, dil, seg); }
cudaError_t mkt_col2im_acc(const float* dcol, float* dx, size_t rows, size_t C, int k, int dil, size_t seg, cudaStream_t st) { L1(k_col2im_acc, rows * C, dcol, dx, rows, C, k, dil, seg); }
cudaError_t mkt_dwconv(const float* x, const float* w, float* y, size_t rows, size_t C, int k, size_t seg, cudaStream_t st) { L1(k_dwconv, rows * C, x, w, y, rows, C, k, seg); }
cudaError_t mkt_dwconv_bwd_x(const float* d, const float* w, float* dx, size_t rows, size_t C, int k, size_t seg, cudaStream_t st) { L1(k_dwconv_bwd_x, rows * C, d, w, dx, rows, C, k, seg); }
cudaError_t mkt_dwconv_bwd_w(const float* d, const float* x, float* dw, size_t rows, size_t C, int k, size_t seg, cudaStream_t st) {
    unsigned gz = (unsigned)((rows + 255) / 256); if (gz > 64) gz = 64; if (gz == 0) gz = 1;
    dim3 grid((unsigned)((C + 31) / 32), (unsigned)k, gz);
    k_dwconv_bwd_w<<<grid, dim3(32, 8), 0, st>>>(d, x, dw, rows, C, k, seg);
    return cudaGetLastError();
}
cudaError_t mkt_layer_norm(const float* x, float* xhat, float* rstd, size_t rows, size_t cols, cudaStream_t st) {
    k_layer_norm<<<(unsigned)rows, cols >= 512 ? 256 : 128, 0, st>>>(x, xhat, rstd, cols);
    return cudaGetLastError();
}
cudaError_t mkt_layer_norm_bwd(const float* d, const float* xhat, const float* rstd, float* dx, size_t rows, size_t cols, cudaStream_t st) {
    k_layer_norm_bwd<<<(unsigned)rows, cols >= 512 ? 256 : 128, 0, st>>>(d, xhat, rstd, dx, cols);
    return cudaGetLastError();
}
cudaError_t mkt_rope(const float* x, float* y, size_t rows, size_t C, int heads, size_t seg, float sign, int acc, cudaStream_t st) {
    L1(k_rope, rows * (size_t)heads * (C / heads / 2), x, y, rows, C, heads, seg, sign, acc);
}
cudaError_t mkt_split_heads(const float* x, float* y, size_t B, size_t T, int H, int dh, cudaStream_t st) { L1(k_split_heads, B * T * (size_t)H * dh, x, y, B, T, H, dh); }
cudaError_t mkt_merge_heads(const float* x, float* y, size_t B, size_t T, int H, int dh, int acc, cudaStream_t st) { L1(k_merge_heads, B * T * (size_t)H * dh, x, y, B, T, H, dh, acc); }
cudaError_t mkt_softmax_rows(float* s, size_t rows, size_t T, size_t S, int H, const uint32_t* key_len, float scale, cudaStream_t st) {
    k_softmax_rows<<<(unsigned)rows, 128, 0, st>>>(s, T, S, H, key_len, scale);
    return cudaGetLastError();
}
cudaError_t mkt_softmax_bwd_rows(const float* p, float* dp, size_t rows, size_t S, float scale, cudaStream_t st) {
    k_softmax_bwd_rows<<<(unsigned)rows, 128, 0, st>>>(p, dp, S, scale);
    return cudaGetLastError();
}
cudaError_t mkt_loss(const float* x, const float* t, const float* w, const uint32_t* col_lim, size_t seg, float* g, float* out, size_t rows, size_t cols, float inv_denom, int sq, cudaStream_t st) {
    unsigned b = mkt_blocks(rows * cols); if (b > 1024) b = 1024;
    k_loss<<<b, MKT_THREADS, 0, st>>>(x, t, w, col_lim, seg, g, out, rows, cols, inv_denom, sq);
    return cudaGetLastError();
}
cudaError_t mkt_scaled_acc(const float* g, const float* d, float* dx, size_t n, cudaStream_t st) { L1(k_scaled_acc, n, g, d, dx, n); }
cudaError_t mkt_bce(const float* x, const float* t, float* g, float* out, size_t n, cudaStream_t st) {
    unsigned b = mkt_blocks(n); if (b > 1024) b = 1024;
    k_bce<<<b, MKT_THREADS, 0, st>>>(x, t, g, out, n);
    return cudaGetLastError();
}
cudaError_t mkt_hn_filter(const float* gh, const float* gn, const float* pa, const float* pb, const float* hr, const float* hi, const float* nr, const float* ni, float* y, size_t rows, size_t F, cudaStream_t st) {
    L1(k_hn_filter, rows * F, gh, gn, pa, pb, hr, hi, nr, ni, y, rows, F);
}
cudaError_t mkt_hn_filter_bwd(const float* d, const float* gh, const float* gn, const float* pa, const float* pb, const float* hr, const float* hi, const float* nr, const float* ni, float* dgh, float* dgn, float* dpa, float* dpb, size_t rows, size_t F, cudaStream_t st) {
    L1(k_hn_filter_bwd, rows * F, d, gh, gn, pa, pb, hr, hi, nr, ni, dgh, dgn, dpa, dpb, rows, F);
}
cudaError_t mkt_fft(float* re, float* im, size_t batch, int n, int inverse, cudaStream_t st) {
    int log2n = 0; while ((1 << log2n) < n) log2n++;
    int threads = n / 2 < 512 ? n / 2 : 512;
    size_t done = 0;
    while (done < batch) {
        size_t chunk = batch - done; if (chunk > 65535) chunk = 65535;
        k_fft<<<(unsigned)chunk, threads, 2 * n * sizeof(float), st>>>(re + done * n, im + done * n, n, log2n, inverse);
        done += chunk;
    }
    return cudaGetLastError();
}
cudaError_t mkt_frame(const float* x, const float* gain, const float* win, float* re, float* im, size_t B, size_t len, size_t frames, int n, int hop, cudaStream_t st) {
    L1(k_frame, B * frames * (size_t)n, x, gain, win, re, im, B, len, frames, n, hop);
}
cudaError_t mkt_half(const float* fr, const float* fi, float* hr, float* hi, size_t rows, int n, float scale, cudaStream_t st) { L1(k_half, rows * (size_t)(n / 2 + 1), fr, fi, hr, hi, rows, n, scale); }
cudaError_t mkt_half_c(const float* fr, const float* fi, float* hr, float* hi, size_t rows, int n, float c_edge, float c_mid, cudaStream_t st) { L1(k_half_c, rows * (size_t)(n / 2 + 1), fr, fi, hr, hi, rows, n, c_edge, c_mid); }
cudaError_t mkt_hermitian(const float* hr, const float* hi, float* fr, float* fi, size_t rows, int n, float c_edge, float c_mid, cudaStream_t st) { L1(k_hermitian, rows * (size_t)n, hr, hi, fr, fi, rows, n, c_edge, c_mid); }
cudaError_t mkt_overlap_add(const float* fr, const float* win, const float* norm, float* y, size_t B, size_t len, size_t frames, int n, int hop, float scale, int acc, cudaStream_t st) {
    L1(k_overlap_add, B * len, fr, win, norm, y, B, len, frames, n, hop, scale, acc);
}
cudaError_t mkt_mag(const float* re, const float* im, float* m, size_t n, cudaStream_t st) { L1(k_mag, n, re, im, m, n); }
cudaError_t mkt_mag_bwd(const float* d, const float* re, const float* im, const float* m, float* dre, float* dim, size_t n, cudaStream_t st) { L1(k_mag_bwd, n, d, re, im, m, dre, dim, n); }
cudaError_t mkt_split_ri(const float* y, float* re, float* im, size_t rows, size_t F, cudaStream_t st) { L1(k_split_ri, rows * F, y, re, im, rows, F); }
cudaError_t mkt_join_ri_acc(const float* re, const float* im, float* y, size_t rows, size_t F, cudaStream_t st) { L1(k_join_ri_acc, rows * F, re, im, y, rows, F); }
cudaError_t mkt_sumsq(const float* g, size_t n, float* out, cudaStream_t st) {
    unsigned b = mkt_blocks(n); if (b > 1024) b = 1024;
    k_sumsq<<<b, MKT_THREADS, 0, st>>>(g, n, out);
    return cudaGetLastError();
}
cudaError_t mkt_adamw(float* p, const float* g, float* m, float* v, size_t n, float lr, float b1, float b2, float eps, float wd, float bc1, float bc2, const float* gscale, cudaStream_t st) {
    L1(k_adamw, n, p, g, m, v, n, lr, b1, b2, eps, wd, bc1, bc2, gscale);
}
cudaError_t mkt_clip_scale(const float* sumsq, float max_norm, float* scale, cudaStream_t st) { k_clip_scale<<<1, 32, 0, st>>>(sumsq, max_norm, scale); return cudaGetLastError(); }
cudaError_t mkt_ema(float* e, const float* p, float decay, size_t n, cudaStream_t st) { L1(k_ema, n, e, p, decay, n); }
cudaError_t mkt_randn(float* y, size_t n, uint64_t seed, cudaStream_t st) { L1(k_randn, n, y, n, seed); }
cudaError_t mkt_mas_logp(float* dot, const float* mu, const float* mel, size_t B, int N, int T, int D, cudaStream_t st) { L1(k_mas_logp, B * (size_t)N * T, dot, mu, mel, B, N, T, D); }
cudaError_t mkt_dur_to_idx(const uint32_t* dur, uint32_t* idx, float* logdur, size_t B, int N, int T, cudaStream_t st) {
    k_dur_to_idx<<<(unsigned)((B + 63) / 64), 64, 0, st>>>(dur, idx, logdur, B, N, T);
    return cudaGetLastError();
}
cudaError_t mkt_harmonic(const float* ph, const float* hz, const float* amp, float* y, size_t n, float top, float level, cudaStream_t st) { L1(k_harmonic, n, ph, hz, amp, y, n, top, level); }
cudaError_t mkt_mas(float* logp, const uint32_t* n_tok, const uint32_t* n_frame, uint32_t* dur, size_t B, int N, int T, cudaStream_t st) {
    k_mas<<<(unsigned)B, 256, 0, st>>>(logp, n_tok, n_frame, dur, N, T);
    return cudaGetLastError();
}
}
