// Bulk dense dequantization kernels: GGUF K-quant / legacy blocks and the
// ComfyUI NVFP4 "pairs" layout -> bf16 row-major scratch, feeding the dense
// cuBLAS linear path (f32-accumulate spine). CPU reference twins live in
// src/quant.rs (dequantize_q4_k / dequantize_q6_k / dequantize_q4_0 /
// dequantize_nvfp4_pairs_row) — keep them bit-identical in structure.
//
// Layout invariant shared with the Rust side: every weight tensor is
// row-major (rows = out features, cols = in features) with cols divisible by
// the block width, so block i of the linear byte stream covers exactly
// output elements [i*block_elems, (i+1)*block_elems).

#include <cuda_runtime.h>
#include <stdint.h>

static __device__ __forceinline__ uint16_t makepad_ggml_kq_f32_to_bf16_bits(float value) {
    const uint32_t bits = __float_as_uint(value);
    return static_cast<uint16_t>(bits >> 16);
}

static __device__ __forceinline__ float makepad_ggml_kq_bf16_bits_to_f32(uint16_t value) {
    return __uint_as_float(static_cast<uint32_t>(value) << 16);
}

static __device__ __forceinline__ float makepad_ggml_kq_f16_bits_to_f32(uint16_t value) {
    const uint32_t sign = (value >> 15) & 1u;
    const uint32_t exp = (value >> 10) & 0x1fu;
    const uint32_t mant = value & 0x3ffu;
    if (exp == 0u) {
        if (mant == 0u) {
            return __uint_as_float(sign << 31);
        }
        uint32_t m = mant;
        int32_t e = 0;
        while ((m & 0x400u) == 0u) {
            m <<= 1;
            e -= 1;
        }
        m &= 0x3ffu;
        const uint32_t exp32 = static_cast<uint32_t>(127 - 15 + 1 + e);
        return __uint_as_float((sign << 31) | (exp32 << 23) | (m << 13));
    }
    if (exp == 31u) {
        return __uint_as_float((sign << 31) | (0xffu << 23) | (mant << 13));
    }
    return __uint_as_float((sign << 31) | ((exp + (127 - 15)) << 23) | (mant << 13));
}

// 6-bit packed scale/min pair of a K-quant super-block (upstream ggml
// get_scale_min_k4).
static __device__ __forceinline__ void makepad_ggml_kq_scale_min_k4(
        uint32_t j,
        const uint8_t * __restrict__ q,
        float * __restrict__ sc,
        float * __restrict__ m) {
    if (j < 4u) {
        *sc = static_cast<float>(q[j] & 63u);
        *m = static_cast<float>(q[j + 4u] & 63u);
    } else {
        *sc = static_cast<float>((q[j + 4u] & 0x0Fu) | ((q[j - 4u] >> 6u) << 4u));
        *m = static_cast<float>((q[j + 4u] >> 4u) | ((q[j] >> 6u) << 4u));
    }
}

// ---------------------------------------------------------------------------
// Q4_K: 144-byte super-block -> 256 bf16 values. One thread per quant byte
// (two output values 32 columns apart), flat over all super-blocks.
// ---------------------------------------------------------------------------

static __global__ void makepad_ggml_kq_dequant_q4_k_bf16_kernel(
        const uint8_t * __restrict__ src,
        uint16_t * __restrict__ dst,
        uint32_t n_super_blocks) {
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    const uint32_t total = n_super_blocks * 128u;
    if (idx >= total) {
        return;
    }
    const uint32_t sb = idx >> 7;
    const uint32_t t = idx & 127u;
    const uint32_t group = t >> 5; // which 64-value pair group (0..3)
    const uint32_t l = t & 31u;
    const uint8_t *block = src + static_cast<size_t>(sb) * 144u;
    const float d = makepad_ggml_kq_f16_bits_to_f32(
        static_cast<uint16_t>(block[0]) | (static_cast<uint16_t>(block[1]) << 8));
    const float dmin = makepad_ggml_kq_f16_bits_to_f32(
        static_cast<uint16_t>(block[2]) | (static_cast<uint16_t>(block[3]) << 8));
    const uint8_t *scales = block + 4;
    const uint8_t q = block[16 + 32 * group + l];
    float sc1, m1, sc2, m2;
    makepad_ggml_kq_scale_min_k4(2u * group, scales, &sc1, &m1);
    makepad_ggml_kq_scale_min_k4(2u * group + 1u, scales, &sc2, &m2);
    uint16_t *out = dst + static_cast<size_t>(sb) * 256u + group * 64u + l;
    out[0] = makepad_ggml_kq_f32_to_bf16_bits(
        d * sc1 * static_cast<float>(q & 0x0Fu) - dmin * m1);
    out[32] = makepad_ggml_kq_f32_to_bf16_bits(
        d * sc2 * static_cast<float>(q >> 4u) - dmin * m2);
}

extern "C" cudaError_t makepad_ggml_cuda_dequant_q4_k_bf16(
        const void *src_blocks,
        void *dst_bf16,
        uint32_t n_super_blocks,
        cudaStream_t stream) {
    if (n_super_blocks == 0) {
        return cudaSuccess;
    }
    const uint32_t total = n_super_blocks * 128u;
    const uint32_t block_dim = 256u;
    const uint32_t grid = (total + block_dim - 1u) / block_dim;
    makepad_ggml_kq_dequant_q4_k_bf16_kernel<<<grid, block_dim, 0, stream>>>(
        static_cast<const uint8_t *>(src_blocks),
        static_cast<uint16_t *>(dst_bf16),
        n_super_blocks);
    return cudaGetLastError();
}

// ---------------------------------------------------------------------------
// Q6_K: 210-byte super-block (ql[128] | qh[64] | scales[16 i8] | d f16) ->
// 256 bf16 values. One thread per (half, l) lane producing 4 values.
// ---------------------------------------------------------------------------

static __global__ void makepad_ggml_kq_dequant_q6_k_bf16_kernel(
        const uint8_t * __restrict__ src,
        uint16_t * __restrict__ dst,
        uint32_t n_super_blocks) {
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    const uint32_t total = n_super_blocks * 64u;
    if (idx >= total) {
        return;
    }
    const uint32_t sb = idx >> 6;
    const uint32_t t = idx & 63u;
    const uint32_t half_idx = t >> 5; // 0 or 1: which 128-value half
    const uint32_t l = t & 31u;
    const uint32_t is = l >> 4;
    const uint8_t *block = src + static_cast<size_t>(sb) * 210u;
    const float d = makepad_ggml_kq_f16_bits_to_f32(
        static_cast<uint16_t>(block[208]) | (static_cast<uint16_t>(block[209]) << 8));
    const uint8_t *ql = block + half_idx * 64u;
    const uint8_t *qh = block + 128u + half_idx * 32u;
    const int8_t *sc = reinterpret_cast<const int8_t *>(block + 192u) + half_idx * 8u;
    const int32_t q1 =
        static_cast<int32_t>(static_cast<int8_t>((ql[l] & 0x0Fu) | ((qh[l] & 3u) << 4u))) - 32;
    const int32_t q2 = static_cast<int32_t>(static_cast<int8_t>(
                           (ql[l + 32u] & 0x0Fu) | (((qh[l] >> 2u) & 3u) << 4u))) -
        32;
    const int32_t q3 = static_cast<int32_t>(
                           static_cast<int8_t>((ql[l] >> 4u) | (((qh[l] >> 4u) & 3u) << 4u))) -
        32;
    const int32_t q4 = static_cast<int32_t>(static_cast<int8_t>(
                           (ql[l + 32u] >> 4u) | (((qh[l] >> 6u) & 3u) << 4u))) -
        32;
    uint16_t *out = dst + static_cast<size_t>(sb) * 256u + half_idx * 128u + l;
    out[0] = makepad_ggml_kq_f32_to_bf16_bits(d * static_cast<float>(sc[is]) * q1);
    out[32] = makepad_ggml_kq_f32_to_bf16_bits(d * static_cast<float>(sc[is + 2]) * q2);
    out[64] = makepad_ggml_kq_f32_to_bf16_bits(d * static_cast<float>(sc[is + 4]) * q3);
    out[96] = makepad_ggml_kq_f32_to_bf16_bits(d * static_cast<float>(sc[is + 6]) * q4);
}

extern "C" cudaError_t makepad_ggml_cuda_dequant_q6_k_bf16(
        const void *src_blocks,
        void *dst_bf16,
        uint32_t n_super_blocks,
        cudaStream_t stream) {
    if (n_super_blocks == 0) {
        return cudaSuccess;
    }
    const uint32_t total = n_super_blocks * 64u;
    const uint32_t block_dim = 256u;
    const uint32_t grid = (total + block_dim - 1u) / block_dim;
    makepad_ggml_kq_dequant_q6_k_bf16_kernel<<<grid, block_dim, 0, stream>>>(
        static_cast<const uint8_t *>(src_blocks),
        static_cast<uint16_t *>(dst_bf16),
        n_super_blocks);
    return cudaGetLastError();
}

// ---------------------------------------------------------------------------
// Q4_0: 18-byte block -> 32 bf16 values. One thread per quant byte.
// ---------------------------------------------------------------------------

static __global__ void makepad_ggml_kq_dequant_q4_0_bf16_kernel(
        const uint8_t * __restrict__ src,
        uint16_t * __restrict__ dst,
        uint32_t n_blocks) {
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    const uint32_t total = n_blocks * 16u;
    if (idx >= total) {
        return;
    }
    const uint32_t b = idx >> 4;
    const uint32_t j = idx & 15u;
    const uint8_t *block = src + static_cast<size_t>(b) * 18u;
    const float d = makepad_ggml_kq_f16_bits_to_f32(
        static_cast<uint16_t>(block[0]) | (static_cast<uint16_t>(block[1]) << 8));
    const uint8_t q = block[2 + j];
    uint16_t *out = dst + static_cast<size_t>(b) * 32u + j;
    out[0] = makepad_ggml_kq_f32_to_bf16_bits(
        d * static_cast<float>(static_cast<int32_t>(q & 0x0Fu) - 8));
    out[16] = makepad_ggml_kq_f32_to_bf16_bits(
        d * static_cast<float>(static_cast<int32_t>(q >> 4u) - 8));
}

extern "C" cudaError_t makepad_ggml_cuda_dequant_q4_0_bf16(
        const void *src_blocks,
        void *dst_bf16,
        uint32_t n_blocks,
        cudaStream_t stream) {
    if (n_blocks == 0) {
        return cudaSuccess;
    }
    const uint32_t total = n_blocks * 16u;
    const uint32_t block_dim = 256u;
    const uint32_t grid = (total + block_dim - 1u) / block_dim;
    makepad_ggml_kq_dequant_q4_0_bf16_kernel<<<grid, block_dim, 0, stream>>>(
        static_cast<const uint8_t *>(src_blocks),
        static_cast<uint16_t *>(dst_bf16),
        n_blocks);
    return cudaGetLastError();
}

// ---------------------------------------------------------------------------
// NVFP4 "pairs" (ComfyUI / TensorRT-ModelOpt): self-describing packed blob
//   [32-byte header | scales u8[rows*cols/16] | weights u8[rows*cols/2]
//    | pre_scale bf16[cols] when flags&1]
// header: magic "NV4P", rows, cols, flags, scale2 f32 (see quant.rs
// h3_nvfp4_pairs_pack). Sequential nibble order: low nibble = even column.
// One thread per weight byte (two adjacent output columns).
// ---------------------------------------------------------------------------

static __device__ __forceinline__ float makepad_ggml_kq_e4m3_scale(uint8_t x) {
    if (x == 0u || x == 0x7fu || x == 0xffu) {
        return 0.0f;
    }
    const int32_t exp = (x >> 3) & 0x0f;
    const int32_t man = x & 0x07;
    if (exp == 0) {
        return static_cast<float>(man) * 0.001953125f; // 2^-9
    }
    return (1.0f + static_cast<float>(man) * 0.125f) * exp2f(static_cast<float>(exp - 7));
}

static __constant__ float makepad_ggml_kq_e2m1_values[16] = {
    0.0f, 0.5f, 1.0f, 1.5f, 2.0f, 3.0f, 4.0f, 6.0f,
    -0.0f, -0.5f, -1.0f, -1.5f, -2.0f, -3.0f, -4.0f, -6.0f,
};

static __global__ void makepad_ggml_kq_dequant_nvfp4_pairs_bf16_kernel(
        const uint8_t * __restrict__ blob,
        uint16_t * __restrict__ dst,
        uint32_t rows,
        uint32_t cols) {
    const uint32_t pairs_per_row = cols >> 1;
    const size_t idx =
        static_cast<size_t>(blockIdx.x) * blockDim.x + static_cast<size_t>(threadIdx.x);
    const size_t total = static_cast<size_t>(rows) * pairs_per_row;
    if (idx >= total) {
        return;
    }
    const uint32_t flags = *reinterpret_cast<const uint32_t *>(blob + 12);
    const float scale2 = *reinterpret_cast<const float *>(blob + 16);
    const uint8_t *scales = blob + 32;
    const uint8_t *qs = scales + static_cast<size_t>(rows) * (cols >> 4);
    const uint16_t *pre = (flags & 1u)
        ? reinterpret_cast<const uint16_t *>(qs + static_cast<size_t>(rows) * pairs_per_row)
        : nullptr;
    const uint32_t row = static_cast<uint32_t>(idx / pairs_per_row);
    const uint32_t pair = static_cast<uint32_t>(idx % pairs_per_row);
    const uint32_t col0 = pair << 1;
    const float d = scale2 *
        makepad_ggml_kq_e4m3_scale(scales[static_cast<size_t>(row) * (cols >> 4) + (col0 >> 4)]);
    const uint8_t packed = qs[idx];
    float v0 = d * makepad_ggml_kq_e2m1_values[packed & 0x0fu];
    float v1 = d * makepad_ggml_kq_e2m1_values[packed >> 4u];
    if (pre != nullptr) {
        v0 *= makepad_ggml_kq_bf16_bits_to_f32(pre[col0]);
        v1 *= makepad_ggml_kq_bf16_bits_to_f32(pre[col0 + 1u]);
    }
    uint16_t *out = dst + static_cast<size_t>(row) * cols + col0;
    out[0] = makepad_ggml_kq_f32_to_bf16_bits(v0);
    out[1] = makepad_ggml_kq_f32_to_bf16_bits(v1);
}

extern "C" cudaError_t makepad_ggml_cuda_dequant_nvfp4_pairs_bf16(
        const void *packed_blob,
        void *dst_bf16,
        uint32_t rows,
        uint32_t cols,
        cudaStream_t stream) {
    if (rows == 0 || cols == 0) {
        return cudaSuccess;
    }
    const size_t total = static_cast<size_t>(rows) * (cols >> 1);
    const uint32_t block_dim = 256u;
    const uint32_t grid = static_cast<uint32_t>((total + block_dim - 1u) / block_dim);
    makepad_ggml_kq_dequant_nvfp4_pairs_bf16_kernel<<<grid, block_dim, 0, stream>>>(
        static_cast<const uint8_t *>(packed_blob),
        static_cast<uint16_t *>(dst_bf16),
        rows,
        cols);
    return cudaGetLastError();
}

// ---------------------------------------------------------------------------
// F8_E4M3 (signed E4M3FN, implicit scale 1.0): raw 1-byte scalars -> bf16 or
// gathered f32 rows. CPU reference twin: quant.rs f8_e4m3_to_f32 — keep the
// decode bit-identical. Every finite E4M3FN value has <= 3 mantissa bits, so
// both the f32 assembly and the bf16 truncation below are EXACT (no
// rounding); 0x7f/0xff assemble to f32/bf16 NaN (loaders reject those bytes
// before upload, fail-closed).
// ---------------------------------------------------------------------------

static __device__ __forceinline__ uint32_t makepad_ggml_kq_f8_e4m3_to_f32_bits(uint8_t v) {
    const uint32_t sign = (static_cast<uint32_t>(v) & 0x80u) << 24;
    const uint32_t exp = (static_cast<uint32_t>(v) >> 3) & 0x0fu;
    const uint32_t man = static_cast<uint32_t>(v) & 0x07u;
    if (exp == 0x0fu && man == 0x07u) {
        return 0x7fc00000u; // E4M3FN NaN (no infinities in this format)
    }
    if (exp == 0u) {
        if (man == 0u) {
            return sign; // +-0.0
        }
        // Subnormal: man * 2^-9. Normalize the 3-bit mantissa into f32.
        uint32_t m = man;
        int32_t shift = 0;
        while ((m & 0x8u) == 0u) {
            m <<= 1;
            shift += 1;
        }
        const uint32_t exp32 = static_cast<uint32_t>(127 - 6 - shift);
        return sign | (exp32 << 23) | ((m & 0x7u) << 20);
    }
    return sign | ((exp + 120u) << 23) | (man << 20);
}

static __global__ void makepad_ggml_kq_dequant_f8_e4m3_bf16_kernel(
        const uint8_t * __restrict__ src,
        uint16_t * __restrict__ dst,
        uint32_t count) {
    const uint32_t base = (blockIdx.x * blockDim.x + threadIdx.x) * 4u;
    if (base >= count) {
        return;
    }
    const uint32_t take = count - base < 4u ? count - base : 4u;
    #pragma unroll
    for (uint32_t i = 0; i < 4u; i++) {
        if (i < take) {
            dst[base + i] = static_cast<uint16_t>(
                makepad_ggml_kq_f8_e4m3_to_f32_bits(src[base + i]) >> 16);
        }
    }
}

extern "C" cudaError_t makepad_ggml_cuda_dequant_f8_e4m3_bf16(
        const void *src_bytes,
        void *dst_bf16,
        uint32_t count,
        cudaStream_t stream) {
    if (count == 0) {
        return cudaSuccess;
    }
    const uint32_t block_dim = 256u;
    const uint32_t quads = (count + 3u) / 4u;
    const uint32_t grid = (quads + block_dim - 1u) / block_dim;
    makepad_ggml_kq_dequant_f8_e4m3_bf16_kernel<<<grid, block_dim, 0, stream>>>(
        static_cast<const uint8_t *>(src_bytes),
        static_cast<uint16_t *>(dst_bf16),
        count);
    return cudaGetLastError();
}

// Gathered embedding rows: out[i][j] = decode(src[row_indices[i]][j]) as f32.
// Row indices were validated in range on the host before upload.

static __global__ void makepad_ggml_kq_get_rows_f8_e4m3_f32_kernel(
        const uint8_t * __restrict__ src,
        const int32_t * __restrict__ row_indices,
        float * __restrict__ dst,
        uint32_t n_cols,
        uint32_t n_take) {
    const uint32_t idx = blockIdx.x * blockDim.x + threadIdx.x;
    const uint32_t total = n_take * n_cols;
    if (idx >= total) {
        return;
    }
    const uint32_t take = idx / n_cols;
    const uint32_t col = idx % n_cols;
    const size_t src_index =
        static_cast<size_t>(row_indices[take]) * n_cols + col;
    dst[idx] = __uint_as_float(makepad_ggml_kq_f8_e4m3_to_f32_bits(src[src_index]));
}

extern "C" cudaError_t makepad_ggml_cuda_get_rows_f8_e4m3_f32(
        const void *src_bytes,
        const void *row_indices_i32,
        void *dst_f32,
        uint32_t n_cols,
        uint32_t n_take,
        cudaStream_t stream) {
    if (n_cols == 0 || n_take == 0) {
        return cudaSuccess;
    }
    const uint32_t total = n_take * n_cols;
    const uint32_t block_dim = 256u;
    const uint32_t grid = (total + block_dim - 1u) / block_dim;
    makepad_ggml_kq_get_rows_f8_e4m3_f32_kernel<<<grid, block_dim, 0, stream>>>(
        static_cast<const uint8_t *>(src_bytes),
        static_cast<const int32_t *>(row_indices_i32),
        static_cast<float *>(dst_f32),
        n_cols,
        n_take);
    return cudaGetLastError();
}
