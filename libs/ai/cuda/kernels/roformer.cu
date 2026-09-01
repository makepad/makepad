// RoFormer (BS-RoFormer / stems) primitives the LLM kernels cannot serve.
//
// Two gaps, both structural rather than incidental:
//
//  1. AXIAL ATTENTION. `mkllm_flash_decode` / `fattn_*` are decode-shaped:
//     they require an f16 KV cache, a mask, and they have no 4th (batch)
//     dimension — the grid is (head, query). BS-RoFormer's attention is the
//     opposite shape: f32 K/V, NO mask, non-causal, head_dim 64, and a real
//     batch axis of 62 bands (time transformer) or 1101 frames (freq
//     transformer). Looping the decode kernel over that axis would be 1101
//     launches of a serial-over-keys kernel per attention node.
//
//  2. ROPE, INTERLEAVED. `mkllm_rope_multi` implements the NeoX/split-half
//     convention (`x[ic]`, `x[ic + n_dims/2]`). BS-RoFormer uses
//     `GGML_ROPE_TYPE_NORMAL` — `rotary_embedding_torch` rotates adjacent
//     pairs `(x[2i], x[2i+1])`. Getting this wrong is a silent quality
//     collapse, not an error, so it gets its own kernel rather than a flag
//     on a kernel whose indexing is the other convention.
//
// Everything here is f32 in, f32 out: the Metal oracle runs this graph with
// `flash_attn_ext_set_prec(F32)`, and the parity gate (~60 dB SNR against the
// PyTorch reference) is measured in that arithmetic.

#include <cuda_runtime.h>
#include <math.h>
#include <stdint.h>

// ---------------------------------------------------------------------------
// Batched, maskless, non-causal attention — FlashAttention-2 tiling in f32.
//
// ggml layout contract (matches ggml_flash_attn_ext exactly):
//   q   ne = [D,  n_q, H,   B]  strides q_nb1 (query), q_nb2 (head), q_nb3 (batch)
//   k   ne = [D,  kc,  Hkv, B]  strides k_nb1 (key),   k_nb2 (head), k_nb3 (batch)
//   v   ne = [D,  kc,  Hkv, B]  strides v_nb1 (key),   v_nb2 (head), v_nb3 (batch)
//   dst ne = [D,  H,   n_q, B]  strides d_nb1 (head),  d_nb2 (query), d_nb3 (batch)
// All strides are BYTES; dim 0 is contiguous f32. Note dst's head/query axes
// are transposed relative to q — that is ggml's convention, not a mistake.
//
// One block owns ROF_BR queries of one (head, batch) and streams the whole key
// axis in ROF_BC-row tiles, keeping the running max/sum per query row and the
// output accumulator in registers (online softmax, never materializing the
// [n_q, kc] score matrix). Template on the head dim so the accumulator array
// is register-resident rather than spilled to local memory.
// ---------------------------------------------------------------------------

#define ROF_BR 32        // query rows per block
#define ROF_BC 32        // key rows per tile iteration
#define ROF_THREADS 128  // 4 threads per query row
#define ROF_SUBS (ROF_THREADS / ROF_BR)
#define ROF_COLS (ROF_BC / ROF_SUBS)

// +1 word of row padding: shared reads walk the row axis at fixed d, so an
// unpadded power-of-two row stride would put all of them in one bank.
#define ROF_LD(d) ((d) + 1)

template <int D>
static __global__ void __launch_bounds__(ROF_THREADS, 2) makepad_cuda_roformer_attn_f32_kernel(
        const uint8_t * __restrict__ q,
        const uint8_t * __restrict__ k,
        const uint8_t * __restrict__ v,
        uint8_t * __restrict__ dst,
        int n_q, int kc, int gqa,
        float scale,
        size_t q_nb1, size_t q_nb2, size_t q_nb3,
        size_t k_nb1, size_t k_nb2, size_t k_nb3,
        size_t v_nb1, size_t v_nb2, size_t v_nb3,
        size_t d_nb1, size_t d_nb2, size_t d_nb3) {
    constexpr int LD = ROF_LD(D);
    constexpr int ACC = D / ROF_SUBS;

    __shared__ float qs[ROF_BR * LD];
    __shared__ float ks[ROF_BC * LD];
    __shared__ float vs[ROF_BC * LD];
    __shared__ float ss[ROF_BR * ROF_BC];
    __shared__ float ms[ROF_BR];
    __shared__ float ls[ROF_BR];
    __shared__ float cs[ROF_BR];

    const int q0 = blockIdx.x * ROF_BR;
    const int head = blockIdx.y;
    const int batch = blockIdx.z;
    const int kv_head = head / gqa;

    const int tid = threadIdx.x;
    const int row = tid / ROF_SUBS;          // query row inside the tile
    const int sub = tid % ROF_SUBS;          // which quarter of the head dim
    const int col0 = sub * ROF_COLS;         // which score columns this thread owns

    const uint8_t * qb = q + (size_t) head * q_nb2 + (size_t) batch * q_nb3;
    const uint8_t * kb = k + (size_t) kv_head * k_nb2 + (size_t) batch * k_nb3;
    const uint8_t * vb = v + (size_t) kv_head * v_nb2 + (size_t) batch * v_nb3;

    // Q tile: rows past the end are zeroed and their results dropped at the
    // store, so the inner loops stay branch-free.
    for (int idx = tid; idx < ROF_BR * D; idx += ROF_THREADS) {
        const int r = idx / D;
        const int d = idx - r * D;
        const int gq = q0 + r;
        qs[r * LD + d] = gq < n_q ? *(const float *) (qb + (size_t) gq * q_nb1 + (size_t) d * 4) : 0.0f;
    }
    if (tid < ROF_BR) {
        ms[tid] = -INFINITY;
        ls[tid] = 0.0f;
    }

    float acc[ACC];
#pragma unroll
    for (int i = 0; i < ACC; i++) {
        acc[i] = 0.0f;
    }

    for (int key0 = 0; key0 < kc; key0 += ROF_BC) {
        __syncthreads();
        for (int idx = tid; idx < ROF_BC * D; idx += ROF_THREADS) {
            const int r = idx / D;
            const int d = idx - r * D;
            const int gk = key0 + r;
            const int live = gk < kc;
            ks[r * LD + d] = live ? *(const float *) (kb + (size_t) gk * k_nb1 + (size_t) d * 4) : 0.0f;
            vs[r * LD + d] = live ? *(const float *) (vb + (size_t) gk * v_nb1 + (size_t) d * 4) : 0.0f;
        }
        __syncthreads();

        // Scores for this thread's ROF_COLS columns of its query row.
        float s[ROF_COLS];
#pragma unroll
        for (int j = 0; j < ROF_COLS; j++) {
            s[j] = 0.0f;
        }
        for (int d = 0; d < D; d++) {
            const float qv = qs[row * LD + d];
#pragma unroll
            for (int j = 0; j < ROF_COLS; j++) {
                s[j] = fmaf(qv, ks[(col0 + j) * LD + d], s[j]);
            }
        }
#pragma unroll
        for (int j = 0; j < ROF_COLS; j++) {
            // Keys past the end must not enter the softmax at all.
            ss[row * ROF_BC + col0 + j] =
                (key0 + col0 + j) < kc ? s[j] * scale : -INFINITY;
        }
        __syncthreads();

        // Online softmax rescale, one thread per query row. The tile is only
        // entered when it holds at least one live key, so m_new is finite and
        // the first-tile `exp(-inf - m_new)` is a clean 0 rather than a NaN.
        if (tid < ROF_BR) {
            const float m_old = ms[tid];
            float m_new = m_old;
#pragma unroll 8
            for (int c = 0; c < ROF_BC; c++) {
                m_new = fmaxf(m_new, ss[tid * ROF_BC + c]);
            }
            const float corr = expf(m_old - m_new);
            float l = ls[tid] * corr;
#pragma unroll 8
            for (int c = 0; c < ROF_BC; c++) {
                const float p = expf(ss[tid * ROF_BC + c] - m_new);
                ss[tid * ROF_BC + c] = p;
                l += p;
            }
            ms[tid] = m_new;
            ls[tid] = l;
            cs[tid] = corr;
        }
        __syncthreads();

        const float corr = cs[row];
#pragma unroll
        for (int i = 0; i < ACC; i++) {
            acc[i] *= corr;
        }
        for (int c = 0; c < ROF_BC; c++) {
            const float p = ss[row * ROF_BC + c];
            const float * vrow = &vs[c * LD + sub * ACC];
#pragma unroll
            for (int i = 0; i < ACC; i++) {
                acc[i] = fmaf(p, vrow[i], acc[i]);
            }
        }
    }

    const int gq = q0 + row;
    if (gq >= n_q) {
        return;
    }
    const float l = ls[row];
    const float inv = l > 0.0f ? 1.0f / l : 0.0f;
    float * out = (float *) (dst + (size_t) batch * d_nb3 + (size_t) gq * d_nb2
        + (size_t) head * d_nb1) + sub * ACC;
#pragma unroll
    for (int i = 0; i < ACC; i++) {
        out[i] = acc[i] * inv;
    }
}

extern "C" cudaError_t makepad_cuda_roformer_attn_f32(
        const void * q, const void * k, const void * v, void * dst,
        int d, int n_q, int kc, int heads, int kv_heads, int batch, float scale,
        size_t q_nb1, size_t q_nb2, size_t q_nb3,
        size_t k_nb1, size_t k_nb2, size_t k_nb3,
        size_t v_nb1, size_t v_nb2, size_t v_nb3,
        size_t d_nb1, size_t d_nb2, size_t d_nb3,
        cudaStream_t stream) {
    if (n_q <= 0 || kc <= 0 || heads <= 0 || kv_heads <= 0 || batch <= 0) {
        return cudaErrorInvalidValue;
    }
    if (kv_heads > heads || heads % kv_heads != 0) {
        return cudaErrorInvalidValue;
    }
    const int gqa = heads / kv_heads;
    const dim3 block(ROF_THREADS, 1, 1);
    const dim3 grid((n_q + ROF_BR - 1) / ROF_BR, heads, batch);

#define ROF_LAUNCH(DIM)                                                        \
    makepad_cuda_roformer_attn_f32_kernel<DIM><<<grid, block, 0, stream>>>(     \
        (const uint8_t *) q, (const uint8_t *) k, (const uint8_t *) v,          \
        (uint8_t *) dst, n_q, kc, gqa, scale,                                   \
        q_nb1, q_nb2, q_nb3, k_nb1, k_nb2, k_nb3,                               \
        v_nb1, v_nb2, v_nb3, d_nb1, d_nb2, d_nb3)

    // Only the head dims this path is verified on. A new one must be added
    // here deliberately (and re-measured against the oracle) rather than
    // silently falling through to a generic slow path. 64 is BS-RoFormer's.
    // Note the ceiling: three (BR x D) f32 tiles plus the score tile must fit
    // the 48 KB static shared-memory budget, which D=128 blows at BC=32 —
    // adding it means re-tiling, not just another case label.
    // 72 is the Qwen3-VL vision tower (n_embd 1152 / 16 heads); its three
    // (32 x 73) f32 tiles plus the score tile come to ~31.8 KB, inside the
    // budget. D must stay divisible by ROF_SUBS (4): 72 / 4 = 18.
    switch (d) {
        case 32: ROF_LAUNCH(32); break;
        case 64: ROF_LAUNCH(64); break;
        case 72: ROF_LAUNCH(72); break;
        default: return cudaErrorInvalidValue;
    }
#undef ROF_LAUNCH
    return cudaGetLastError();
}

// ---------------------------------------------------------------------------
// The same maskless attention, register-tiled for a LONG key axis.
//
// The kernel above was written for BS-RoFormer, whose key axis is at most a
// few hundred entries; it gives each thread ONE query row and four score
// columns, so its inner loops read shared memory about as often as they
// multiply: 9 LDS per 8 FMA in QK, 19 per 18 in PV. An SM retires 128 f32
// FMA per clock but only 32 shared-memory words, so at ~1.1 loads per FMA
// that kernel is shared-bandwidth bound by roughly 4x and can never exceed a
// quarter of peak no matter how many blocks are resident.
//
// The Qwen3-VL vision tower runs the same op at a completely different size:
// 14k-24k patches attend to each other with no mask, which is ~65 TFLOP per
// image at 24k patches and utterly dominates the encode. There the fix is
// register blocking. Each thread owns a 4x4 patch of the score tile (8 loads
// per 16 FMA) and a 4-row x D/8-column patch of the output accumulator (13
// loads per 36 FMA), which is a 2.5x cut in shared traffic per multiply, and
// the online-softmax rescale is spread over all 128 threads (two per query
// row, combined with one shuffle) instead of running on 32 of them while the
// other 96 wait at a barrier.
//
// Everything else — f32 throughout, the online softmax, the tile order, the
// -INFINITY padding of dead columns — is identical to the kernel above, so
// the two agree to rounding. Selection is by shape at the call site: the
// stems lane never reaches this one.
//
// Tiles: 64 queries x 32 keys per iteration, 128 threads.
//   QK phase:  128 = 16 row groups x 8 column groups, 4 rows x 4 cols each.
//   softmax:   128 = 64 rows x 2 halves, 16 columns each.
//   PV phase:  128 = 16 row groups x 8 dim groups, 4 rows x D/8 dims each.
// D must therefore divide by 8 (32, 64 and 72 all do).
// ---------------------------------------------------------------------------

#define ROT_BR 64       // query rows per block
#define ROT_BC 32       // key rows per tile iteration
#define ROT_THREADS 128
#define ROT_RG 16       // row groups
#define ROT_RPT (ROT_BR / ROT_RG)   // 4 query rows per thread
#define ROT_CG 8        // column groups (QK) / dim groups (PV)
#define ROT_CPT (ROT_BC / ROT_CG)   // 4 score columns per thread

// Score-tile row stride. 33 rather than 32 so that the PV phase's
// `ss[(r0+a)*SLD + c]`, whose threads walk r0 in steps of 4 rows, lands on
// four distinct banks instead of all 16 row groups colliding on one.
#define ROT_SLD (ROT_BC + 1)

template <int D>
static __global__ void __launch_bounds__(ROT_THREADS, 2) makepad_cuda_roformer_attn_f32_tiled_kernel(
        const uint8_t * __restrict__ q,
        const uint8_t * __restrict__ k,
        const uint8_t * __restrict__ v,
        uint8_t * __restrict__ dst,
        int n_q, int kc, int gqa,
        float scale,
        size_t q_nb1, size_t q_nb2, size_t q_nb3,
        size_t k_nb1, size_t k_nb2, size_t k_nb3,
        size_t v_nb1, size_t v_nb2, size_t v_nb3,
        size_t d_nb1, size_t d_nb2, size_t d_nb3) {
    constexpr int LD = ROF_LD(D);
    constexpr int DPT = D / ROT_CG;   // accumulator dims per thread

    __shared__ float qs[ROT_BR * LD];
    __shared__ float ks[ROT_BC * LD];
    __shared__ float vs[ROT_BC * LD];
    __shared__ float ss[ROT_BR * ROT_SLD];
    __shared__ float ms[ROT_BR];
    __shared__ float ls[ROT_BR];
    __shared__ float cs[ROT_BR];

    const int q0 = blockIdx.x * ROT_BR;
    const int head = blockIdx.y;
    const int batch = blockIdx.z;
    const int kv_head = head / gqa;

    const int tid = threadIdx.x;
    const int rg = tid / ROT_CG;     // row group, shared by the QK and PV phases
    const int cg = tid % ROT_CG;     // column group (QK) and dim group (PV)
    const int r0 = rg * ROT_RPT;     // first query row this thread owns
    const int c0 = cg * ROT_CPT;     // first score column
    const int d0 = cg * DPT;         // first accumulator dim

    const uint8_t * qb = q + (size_t) head * q_nb2 + (size_t) batch * q_nb3;
    const uint8_t * kb = k + (size_t) kv_head * k_nb2 + (size_t) batch * k_nb3;
    const uint8_t * vb = v + (size_t) kv_head * v_nb2 + (size_t) batch * v_nb3;

    // Q tile: rows past the end are zeroed and their results dropped at the
    // store, so the inner loops stay branch-free.
    for (int idx = tid; idx < ROT_BR * D; idx += ROT_THREADS) {
        const int r = idx / D;
        const int d = idx - r * D;
        const int gq = q0 + r;
        qs[r * LD + d] = gq < n_q ? *(const float *) (qb + (size_t) gq * q_nb1 + (size_t) d * 4) : 0.0f;
    }
    if (tid < ROT_BR) {
        ms[tid] = -INFINITY;
        ls[tid] = 0.0f;
    }

    float acc[ROT_RPT][DPT];
#pragma unroll
    for (int a = 0; a < ROT_RPT; a++) {
#pragma unroll
        for (int b = 0; b < DPT; b++) {
            acc[a][b] = 0.0f;
        }
    }

    for (int key0 = 0; key0 < kc; key0 += ROT_BC) {
        __syncthreads();
        for (int idx = tid; idx < ROT_BC * D; idx += ROT_THREADS) {
            const int r = idx / D;
            const int d = idx - r * D;
            const int gk = key0 + r;
            const int live = gk < kc;
            ks[r * LD + d] = live ? *(const float *) (kb + (size_t) gk * k_nb1 + (size_t) d * 4) : 0.0f;
            vs[r * LD + d] = live ? *(const float *) (vb + (size_t) gk * v_nb1 + (size_t) d * 4) : 0.0f;
        }
        __syncthreads();

        // QK^T, 4x4 per thread: two register vectors feed sixteen multiplies.
        float s[ROT_RPT][ROT_CPT];
#pragma unroll
        for (int a = 0; a < ROT_RPT; a++) {
#pragma unroll
            for (int b = 0; b < ROT_CPT; b++) {
                s[a][b] = 0.0f;
            }
        }
        for (int d = 0; d < D; d++) {
            float qv[ROT_RPT];
            float kv[ROT_CPT];
#pragma unroll
            for (int a = 0; a < ROT_RPT; a++) {
                qv[a] = qs[(r0 + a) * LD + d];
            }
#pragma unroll
            for (int b = 0; b < ROT_CPT; b++) {
                kv[b] = ks[(c0 + b) * LD + d];
            }
#pragma unroll
            for (int a = 0; a < ROT_RPT; a++) {
#pragma unroll
                for (int b = 0; b < ROT_CPT; b++) {
                    s[a][b] = fmaf(qv[a], kv[b], s[a][b]);
                }
            }
        }
#pragma unroll
        for (int a = 0; a < ROT_RPT; a++) {
#pragma unroll
            for (int b = 0; b < ROT_CPT; b++) {
                // Keys past the end must not enter the softmax at all.
                ss[(r0 + a) * ROT_SLD + c0 + b] =
                    (key0 + c0 + b) < kc ? s[a][b] * scale : -INFINITY;
            }
        }
        __syncthreads();

        // Online softmax, two threads per query row. `key0 < kc` guarantees
        // at least one live column, so m_new is finite and the first tile's
        // exp(-inf - m_new) is a clean 0 rather than a NaN. The paired lanes
        // differ only in bit 0, so one shuffle combines both halves; the
        // shuffle also reconverges them, which is what makes the later
        // half==0 store to ms[] safe against the half==1 load above it.
        {
            const int row = tid >> 1;
            const int half = tid & 1;
            const int cbeg = half * (ROT_BC / 2);
            float * srow = &ss[row * ROT_SLD];
            const float m_old = ms[row];
            float m_part = -INFINITY;
#pragma unroll
            for (int c = 0; c < ROT_BC / 2; c++) {
                m_part = fmaxf(m_part, srow[cbeg + c]);
            }
            const float m_new = fmaxf(m_old, fmaxf(m_part, __shfl_xor_sync(0xffffffff, m_part, 1)));
#pragma unroll
            for (int c = 0; c < ROT_BC / 2; c++) {
                srow[cbeg + c] = expf(srow[cbeg + c] - m_new);
            }
            __syncwarp();
            if (half == 0) {
                // The running denominator is summed by one thread, column 0
                // upward, which is the order the kernel above uses. Combining
                // two half-row partials instead would be one ulp out, and a
                // ViT's residual stream turns one ulp at block 0 into
                // something the tower's parity gate can see by block 24 —
                // measured 3.4e-4 relative RMS on the output embeddings,
                // enough to flip a near-tied token and send a long
                // transcription down a different path. Every other operation
                // here already runs in the reference order, so keeping this
                // one makes the two kernels agree bit for bit. The
                // exponentials, which are the expensive half, stay parallel.
                const float corr = expf(m_old - m_new);
                float l = ls[row] * corr;
#pragma unroll 8
                for (int c = 0; c < ROT_BC; c++) {
                    l += srow[c];
                }
                ms[row] = m_new;
                ls[row] = l;
                cs[row] = corr;
            }
        }
        __syncthreads();

        // P*V, 4 rows x DPT dims per thread: four probabilities and DPT value
        // words feed 4*DPT multiplies.
        float corr[ROT_RPT];
#pragma unroll
        for (int a = 0; a < ROT_RPT; a++) {
            corr[a] = cs[r0 + a];
        }
#pragma unroll
        for (int a = 0; a < ROT_RPT; a++) {
#pragma unroll
            for (int b = 0; b < DPT; b++) {
                acc[a][b] *= corr[a];
            }
        }
        for (int c = 0; c < ROT_BC; c++) {
            float pv[ROT_RPT];
#pragma unroll
            for (int a = 0; a < ROT_RPT; a++) {
                pv[a] = ss[(r0 + a) * ROT_SLD + c];
            }
            const float * vrow = &vs[c * LD + d0];
            float vv[DPT];
#pragma unroll
            for (int b = 0; b < DPT; b++) {
                vv[b] = vrow[b];
            }
#pragma unroll
            for (int a = 0; a < ROT_RPT; a++) {
#pragma unroll
                for (int b = 0; b < DPT; b++) {
                    acc[a][b] = fmaf(pv[a], vv[b], acc[a][b]);
                }
            }
        }
    }

#pragma unroll
    for (int a = 0; a < ROT_RPT; a++) {
        const int gq = q0 + r0 + a;
        if (gq >= n_q) {
            continue;
        }
        const float l = ls[r0 + a];
        const float inv = l > 0.0f ? 1.0f / l : 0.0f;
        float * out = (float *) (dst + (size_t) batch * d_nb3 + (size_t) gq * d_nb2
            + (size_t) head * d_nb1) + d0;
#pragma unroll
        for (int b = 0; b < DPT; b++) {
            out[b] = acc[a][b] * inv;
        }
    }
}

extern "C" cudaError_t makepad_cuda_roformer_attn_f32_tiled(
        const void * q, const void * k, const void * v, void * dst,
        int d, int n_q, int kc, int heads, int kv_heads, int batch, float scale,
        size_t q_nb1, size_t q_nb2, size_t q_nb3,
        size_t k_nb1, size_t k_nb2, size_t k_nb3,
        size_t v_nb1, size_t v_nb2, size_t v_nb3,
        size_t d_nb1, size_t d_nb2, size_t d_nb3,
        cudaStream_t stream) {
    if (n_q <= 0 || kc <= 0 || heads <= 0 || kv_heads <= 0 || batch <= 0) {
        return cudaErrorInvalidValue;
    }
    if (kv_heads > heads || heads % kv_heads != 0) {
        return cudaErrorInvalidValue;
    }
    const int gqa = heads / kv_heads;
    const dim3 block(ROT_THREADS, 1, 1);
    const dim3 grid((n_q + ROT_BR - 1) / ROT_BR, heads, batch);

#define ROT_LAUNCH(DIM)                                                        \
    makepad_cuda_roformer_attn_f32_tiled_kernel<DIM><<<grid, block, 0, stream>>>( \
        (const uint8_t *) q, (const uint8_t *) k, (const uint8_t *) v,          \
        (uint8_t *) dst, n_q, kc, gqa, scale,                                   \
        q_nb1, q_nb2, q_nb3, k_nb1, k_nb2, k_nb3,                               \
        v_nb1, v_nb2, v_nb3, d_nb1, d_nb2, d_nb3)

    // Same verified-dims-only rule as the kernel above. D must divide by
    // ROT_CG (8) for the accumulator split, which is the extra constraint
    // here; 72 (Qwen3-VL: n_embd 1152 / 16 heads) is the shape this exists
    // for. Shared at D=72 is 64*73 + 2*32*73 + 64*33 + 3*64 words = 45.5 KB,
    // inside the 48 KB static budget and small enough for two resident
    // blocks out of Ada's 100 KB per SM.
    switch (d) {
        case 32: ROT_LAUNCH(32); break;
        case 64: ROT_LAUNCH(64); break;
        case 72: ROT_LAUNCH(72); break;
        default: return cudaErrorInvalidValue;
    }
#undef ROT_LAUNCH
    return cudaGetLastError();
}

// ---------------------------------------------------------------------------
// RoPE, GGML_ROPE_TYPE_NORMAL: interleaved adjacent pairs (GPT-J style).
//
//   theta(i0) = freq_scale * pos[i2] * freq_base^(-i0 / n_dims)
//   (x[i0], x[i0+1]) <- (x0*cos - x1*sin, x0*sin + x1*cos)
//
// Elements at or beyond n_dims pass through unrotated, exactly as ggml's
// rope_norm does. src/dst carry independent 4D byte strides; `pos` is one
// i32 per ne[2] entry (the sequence axis).
// ---------------------------------------------------------------------------

static __global__ void makepad_cuda_roformer_rope_normal_f32_kernel(
        const uint8_t * __restrict__ src, const int32_t * __restrict__ pos,
        uint8_t * __restrict__ dst,
        int ne0, int n_dims, float freq_base, float freq_scale,
        size_t s_nb0, size_t s_nb1, size_t s_nb2, size_t s_nb3,
        size_t d_nb0, size_t d_nb1, size_t d_nb2, size_t d_nb3) {
    const int i1 = blockIdx.x;
    const int i2 = blockIdx.y;
    const int i3 = blockIdx.z;

    const uint8_t * srow = src + (size_t) i3 * s_nb3 + (size_t) i2 * s_nb2 + (size_t) i1 * s_nb1;
    uint8_t * drow = dst + (size_t) i3 * d_nb3 + (size_t) i2 * d_nb2 + (size_t) i1 * d_nb1;

    const float theta_base = (float) pos[i2];
    const float inv_ndims = -1.0f / (float) n_dims;

    for (int i0 = 2 * threadIdx.x; i0 < ne0; i0 += 2 * blockDim.x) {
        const float x0 = *(const float *) (srow + (size_t) i0 * s_nb0);
        const float x1 = *(const float *) (srow + (size_t) (i0 + 1) * s_nb0);
        float * d0 = (float *) (drow + (size_t) i0 * d_nb0);
        float * d1 = (float *) (drow + (size_t) (i0 + 1) * d_nb0);
        if (i0 < n_dims) {
            const float theta =
                freq_scale * theta_base * powf(freq_base, inv_ndims * (float) i0);
            const float cos_t = cosf(theta);
            const float sin_t = sinf(theta);
            *d0 = x0 * cos_t - x1 * sin_t;
            *d1 = x0 * sin_t + x1 * cos_t;
        } else {
            *d0 = x0;
            *d1 = x1;
        }
    }
}

extern "C" cudaError_t makepad_cuda_roformer_rope_normal_f32(
        const void * src, const int32_t * pos, void * dst,
        int ne0, int ne1, int ne2, int ne3, int n_dims,
        float freq_base, float freq_scale,
        size_t s_nb0, size_t s_nb1, size_t s_nb2, size_t s_nb3,
        size_t d_nb0, size_t d_nb1, size_t d_nb2, size_t d_nb3,
        cudaStream_t stream) {
    if (ne0 <= 0 || ne1 <= 0 || ne2 <= 0 || ne3 <= 0 || (ne0 & 1) != 0) {
        return cudaErrorInvalidValue;
    }
    const dim3 block(64, 1, 1);
    const dim3 grid(ne1, ne2, ne3);
    makepad_cuda_roformer_rope_normal_f32_kernel<<<grid, block, 0, stream>>>(
        (const uint8_t *) src, pos, (uint8_t *) dst,
        ne0, n_dims, freq_base, freq_scale,
        s_nb0, s_nb1, s_nb2, s_nb3, d_nb0, d_nb1, d_nb2, d_nb3);
    return cudaGetLastError();
}
