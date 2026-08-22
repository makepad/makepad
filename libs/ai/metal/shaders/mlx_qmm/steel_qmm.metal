// Thin host wrappers around MLX v0.31.2 steel qmm_t_impl.
// Official kernel is affine_qmm_t<half, 64, 4, true, false, 32, 32, 32>.
// Staging kernels convert ggml F32 [K,M] / [N,M] to MLX F16 [M,K] / [M,N].

#include "mlx/backend/metal/kernels/utils.h"
#include "mlx/backend/metal/kernels/steel/utils/type_traits.h"
#include "mlx/backend/metal/kernels/steel/gemm/gemm.h"
#include "mlx/backend/metal/kernels/quantized_utils.h"
#include "mlx/backend/metal/kernels/quantized.h"

struct SteelQmmArgs {
    int K;
    int N;
    int M;
};

struct SteelPackAArgs {
    int K;
    int M;
    int src_k_stride;
    int src_m_stride;
};

struct SteelUnpackCArgs {
    int N;
    int M;
    int dst_n_stride;
    int dst_m_stride;
};

kernel void kernel_mlx_steel_qmm_f16(
        constant SteelQmmArgs & args [[buffer(0)]],
        const device uint32_t* w [[buffer(1)]],
        const device half* scales [[buffer(2)]],
        const device half* biases [[buffer(3)]],
        const device half* x [[buffer(4)]],
        device half* y [[buffer(5)]],
        uint3 tid [[threadgroup_position_in_grid]],
        uint lid [[thread_index_in_threadgroup]],
        uint simd_gid [[simdgroup_index_in_threadgroup]],
        uint simd_lid [[thread_index_in_simdgroup]]) {
    constexpr int BM = 32;
    constexpr int BK = 32;
    constexpr int BN = 32;
    constexpr int BK_padded = (BK + 16 / sizeof(half));
    threadgroup half Xs[BM * BK_padded];
    threadgroup half Ws[BN * BK_padded];
    qmm_t_impl<half, 64, 4, true, BM, BK, BN>(
        w,
        scales,
        biases,
        x,
        y,
        Xs,
        Ws,
        args.K,
        args.N,
        args.M,
        args.K,
        tid,
        lid,
        simd_gid,
        simd_lid);
}

kernel void kernel_mlx_steel_qmm_pack_a(
        constant SteelPackAArgs & args [[buffer(0)]],
        device const float * src [[buffer(1)]],
        device half * dst [[buffer(2)]],
        uint2 gid [[thread_position_in_grid]]) {
    const uint m = gid.x;
    const uint k = gid.y;
    if (m >= uint(args.M) || k >= uint(args.K)) {
        return;
    }
    dst[m * uint(args.K) + k] =
        half(src[int(k) * args.src_k_stride + int(m) * args.src_m_stride]);
}

// F32 ggml A is [K,M] with A_mlx[m,k] = src[k + m*K]. Same index as
// BlockLoader<half>(A_f16, ld=K) but values are float.
template <short BROWS, short BCOLS, short dst_ld, short tgp_size>
struct F32ToHalfLoader {
    STEEL_CONST short n_reads = (BCOLS * BROWS) / tgp_size;
    STEEL_CONST short TCOLS = BCOLS / n_reads;
    STEEL_CONST short TROWS = tgp_size / TCOLS;
    const int src_ld;
    const int tile_stride;
    const short thread_idx;
    const short bi;
    const short bj;
    threadgroup half* dst;
    const device float* src;

    METAL_FUNC F32ToHalfLoader(
            const device float* src_,
            const int src_ld_,
            threadgroup half* dst_,
            ushort simd_group_id,
            ushort simd_lane_id)
        : src_ld(src_ld_),
          tile_stride(BCOLS),
          thread_idx(simd_group_id * 32 + simd_lane_id),
          bi(thread_idx / TCOLS),
          bj(n_reads * (thread_idx % TCOLS)),
          dst(dst_ + bi * dst_ld + bj),
          src(src_ + bi * src_ld + bj) {}

    METAL_FUNC void load_unsafe() const {
        STEEL_PRAGMA_UNROLL
        for (short i = 0; i < BROWS; i += TROWS) {
            const device float4* s4 =
                (const device float4*)(src + i * src_ld);
            STEEL_PRAGMA_UNROLL
            for (short j = 0; j < n_reads; j += 4) {
                const float4 v = s4[j / 4];
                dst[i * dst_ld + j] = half(v[0]);
                dst[i * dst_ld + j + 1] = half(v[1]);
                dst[i * dst_ld + j + 2] = half(v[2]);
                dst[i * dst_ld + j + 3] = half(v[3]);
            }
        }
    }

    METAL_FUNC void load_safe(short2 src_tile_dim) const {
        src_tile_dim = src_tile_dim - short2(bj, bi);
        if (src_tile_dim.x <= 0 || src_tile_dim.y <= 0) {
            STEEL_PRAGMA_UNROLL
            for (short i = 0; i < BROWS; i += TROWS) {
                STEEL_PRAGMA_UNROLL
                for (short j = 0; j < n_reads; j++) {
                    dst[i * dst_ld + j] = half(0);
                }
            }
            return;
        }
        STEEL_PRAGMA_UNROLL
        for (short i = 0; i < BROWS; i += TROWS) {
            STEEL_PRAGMA_UNROLL
            for (short j = 0; j < n_reads; j++) {
                const bool ok = (i < src_tile_dim.y) && (j < src_tile_dim.x);
                dst[i * dst_ld + j] = ok ? half(src[i * src_ld + j]) : half(0);
            }
        }
    }

    METAL_FUNC void next() { src += tile_stride; }
};

struct SteelQmmF32Args {
    int K;
    int N;
    int M;
    int src_m_stride;
    int dst_m_stride;
};

// Steel BlockMMA + QuantizedBlockLoader, F32 ggml A/C (same index as MLX
// [M,K]/[M,N]). One dispatch — no pack_a/unpack_c.
kernel void kernel_mlx_steel_qmm_f32io(
        constant SteelQmmF32Args & args [[buffer(0)]],
        const device uint32_t* w [[buffer(1)]],
        const device half* scales [[buffer(2)]],
        const device half* biases [[buffer(3)]],
        const device float* x [[buffer(4)]],
        device float* y [[buffer(5)]],
        uint3 tid [[threadgroup_position_in_grid]],
        uint lid [[thread_index_in_threadgroup]],
        uint simd_gid [[simdgroup_index_in_threadgroup]],
        uint simd_lid [[thread_index_in_simdgroup]]) {
    (void)lid;
    constexpr int BM = 32;
    constexpr int BK = 32;
    constexpr int BN = 32;
    constexpr int WM = 2;
    constexpr int WN = 2;
    constexpr int bits = 4;
    constexpr int group_size = 64;
    constexpr int pack_factor = 2;
    constexpr int bytes_per_pack = 1;
    constexpr int BK_padded = (BK + 16 / sizeof(half));
    constexpr int SIMD_SIZE = 32;

    using mma_t = mlx::steel::BlockMMA<
        half, float, BM, BN, BK, WM, WN, false, true, BK_padded, BK_padded>;
    using loader_x_t = F32ToHalfLoader<BM, BK, BK_padded, WM * WN * SIMD_SIZE>;
    using loader_w_t = QuantizedBlockLoader<
        half, BN, BK, BK_padded, 1, WM * WN * SIMD_SIZE, group_size, bits>;

    threadgroup half Xs[BM * BK_padded];
    threadgroup half Ws[BN * BK_padded];

    const int K_w = args.K * bytes_per_pack / pack_factor;
    const int K_g = args.K / group_size;
    const int y_row = int(tid.y) * BM;
    const int y_col = int(tid.x) * BN;

    const device float* x_row = x + y_row * args.src_m_stride;
    auto wl = (const device uint8_t*)w + y_col * K_w;
    const device half* s_row = scales + y_col * K_g;
    const device half* b_row = biases + y_col * K_g;
    device float* y_row_ptr = y + y_row * args.dst_m_stride + y_col;

    const short num_els = min(BM, args.M - y_row);
    const short num_outs = min(BN, args.N - y_col);
    loader_x_t loader_x(x_row, args.src_m_stride, Xs, simd_gid, simd_lid);
    loader_w_t loader_w(wl, s_row, b_row, args.K, Ws, simd_gid, simd_lid);
    mma_t mma_op(simd_gid, simd_lid);

    const bool aligned_n = true;
    if (num_els < BM) {
        for (int k = 0; k < args.K; k += BK) {
            threadgroup_barrier(mem_flags::mem_threadgroup);
            loader_x.load_safe(short2(BK, num_els));
            loader_w.load_unsafe();
            threadgroup_barrier(mem_flags::mem_threadgroup);
            mma_op.mma(Xs, Ws);
            loader_x.next();
            loader_w.next();
        }
    } else {
        for (int k = 0; k < args.K; k += BK) {
            threadgroup_barrier(mem_flags::mem_threadgroup);
            loader_x.load_unsafe();
            loader_w.load_unsafe();
            threadgroup_barrier(mem_flags::mem_threadgroup);
            mma_op.mma(Xs, Ws);
            loader_x.next();
            loader_w.next();
        }
    }
    (void)aligned_n;
    threadgroup_barrier(mem_flags::mem_threadgroup);
    if (num_els < BM || num_outs < BN) {
        mma_op.store_result_safe(y_row_ptr, args.dst_m_stride, short2(num_outs, num_els));
    } else {
        mma_op.store_result(y_row_ptr, args.dst_m_stride);
    }
}

kernel void kernel_mlx_steel_qmm_unpack_c(
        constant SteelUnpackCArgs & args [[buffer(0)]],
        device const half * src [[buffer(1)]],
        device float * dst [[buffer(2)]],
        uint2 gid [[thread_position_in_grid]]) {
    const uint m = gid.x;
    const uint n = gid.y;
    if (m >= uint(args.M) || n >= uint(args.N)) {
        return;
    }
    dst[int(n) * args.dst_n_stride + int(m) * args.dst_m_stride] =
        float(src[m * uint(args.N) + n]);
}
