#include <cuda_runtime.h>
#include <stdint.h>
#include "launch_profile.cuh"

// Row-major bf16 table -> f32 rows. Indices stay on device so the same
// operation can be captured when the preceding kernel produces the id.
static __global__ void gather_rows_bf16_f32_kernel(
    const uint16_t *table, const uint32_t *indices, float *out,
    uint32_t table_rows, uint32_t cols) {
    const uint32_t row = blockIdx.x;
    const uint32_t index = indices[row];
    for (uint32_t col = threadIdx.x; col < cols; col += blockDim.x) {
        // Callers validate ids before upload. Keep a bad device index from
        // reading outside the table, and make it visible in the result.
        out[(size_t)row * cols + col] = index < table_rows
            ? __uint_as_float((uint32_t)table[(size_t)index * cols + col] << 16)
            : __uint_as_float(0x7fc00000u);
    }
}

extern "C" cudaError_t makepad_cuda_gather_rows_bf16_f32(
    const uint16_t *table, const uint32_t *indices, float *out,
    uint32_t table_rows, uint32_t cols, uint32_t count, cudaStream_t stream) {
    if (!table || !indices || !out || !table_rows || !cols || !count) {
        return cudaErrorInvalidValue;
    }
    MakepadLaunchTiming timing("gather_bf16", stream, count, cols);
    gather_rows_bf16_f32_kernel<<<count, 256, 0, stream>>>(
        table, indices, out, table_rows, cols);
    return cudaGetLastError();
}
