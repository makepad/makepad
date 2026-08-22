// Practical-RIFE v4.26 (IFNet_HDv3) kernels.
//
// Only the four ops the released graph needs that no other family already
// had live here: the backward warp (grid_sample bilinear / border /
// align_corners=True), a generic ConvTranspose2d, the ResConv epilogue, and
// the sigmoid merge that also crops the padding and writes the artifact's
// RGB8 bytes. Everything else (strided conv2d, bilinear resize with the
// align_corners switch, pixel shuffle, row slice/concat) is reused from the
// existing store — see makepad-ai-rife's rife_model.rs.
//
// All tensors are planar [C, H*W], batch 1, f32: exactly the layout and the
// precision the portable reference in makepad-ai-rife/src/rife_cpu.rs pins,
// so the two forwards stay comparable.

#include <cuda_runtime.h>
#include <math_constants.h>
#include <stdint.h>

// out[c][y][x] = bilinear(in[c], x + flow[0][y][x], y + flow[1][y][x])
//
// RIFE normalizes the flow onto a linspace(-1, 1) grid and grid_sample
// unnormalizes it again; the two cancel exactly, leaving a plain pixel
// offset. `border` padding is the clamp of the continuous source coordinate:
// once clamped into [0, size-1] the far tap either sits in range or carries
// weight 0, which is what PyTorch's guarded gather computes.
static __global__ void makepad_cuda_rife_warp_f32_kernel(
        const float * __restrict__ input,
        const float * __restrict__ flow,
        float * __restrict__ output,
        uint32_t width,
        uint32_t height,
        uint32_t channels) {
    const size_t plane = static_cast<size_t>(width) * height;
    const size_t i = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    if (i >= plane) return;
    const uint32_t py = static_cast<uint32_t>(i / width);
    const uint32_t px = static_cast<uint32_t>(i - static_cast<size_t>(py) * width);
    const float max_x = static_cast<float>(width - 1u);
    const float max_y = static_cast<float>(height - 1u);
    float fx = static_cast<float>(px) + flow[i];
    float fy = static_cast<float>(py) + flow[plane + i];
    fx = fminf(fmaxf(fx, 0.0f), max_x);
    fy = fminf(fmaxf(fy, 0.0f), max_y);
    const uint32_t x0 = static_cast<uint32_t>(floorf(fx));
    const uint32_t y0 = static_cast<uint32_t>(floorf(fy));
    const uint32_t x1 = min(x0 + 1u, width - 1u);
    const uint32_t y1 = min(y0 + 1u, height - 1u);
    const float lx = fx - static_cast<float>(x0);
    const float ly = fy - static_cast<float>(y0);
    const size_t row0 = static_cast<size_t>(y0) * width;
    const size_t row1 = static_cast<size_t>(y1) * width;
    for (uint32_t c = 0; c < channels; ++c) {
        const float * src = input + static_cast<size_t>(c) * plane;
        const float top = src[row0 + x0] * (1.0f - lx) + src[row0 + x1] * lx;
        const float bot = src[row1 + x0] * (1.0f - lx) + src[row1 + x1] * lx;
        output[static_cast<size_t>(c) * plane + i] = top * (1.0f - ly) + bot * ly;
    }
}

extern "C" cudaError_t makepad_cuda_rife_warp_f32(
        const float * input,
        const float * flow,
        float * output,
        uint32_t width,
        uint32_t height,
        uint32_t channels,
        cudaStream_t stream) {
    if (!width || !height || !channels) return cudaErrorInvalidValue;
    const size_t plane = static_cast<size_t>(width) * height;
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<unsigned int>((plane + block.x - 1) / block.x), 1, 1);
    makepad_cuda_rife_warp_f32_kernel<<<grid, block, 0, stream>>>(
        input, flow, output, width, height, channels);
    return cudaGetLastError();
}

// nn.ConvTranspose2d, weight layout [in, out, kh, kw], output_padding = 0.
// Written as a gather so the accumulation order is fixed per output pixel:
// walk the kernel taps and keep the ones whose source index lands on the
// stride lattice.
static __global__ void makepad_cuda_rife_conv_transpose2d_f32_kernel(
        const float * __restrict__ input,
        const float * __restrict__ weight,
        const float * __restrict__ bias,
        float * __restrict__ output,
        uint32_t in_width,
        uint32_t in_height,
        uint32_t out_width,
        uint32_t out_height,
        uint32_t in_channels,
        uint32_t out_channels,
        uint32_t kw,
        uint32_t kh,
        uint32_t pad,
        uint32_t stride) {
    const size_t out_plane = static_cast<size_t>(out_width) * out_height;
    const size_t i = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    if (i >= out_plane * out_channels) return;
    const uint32_t oc = static_cast<uint32_t>(i / out_plane);
    const size_t p = i - static_cast<size_t>(oc) * out_plane;
    const uint32_t oy = static_cast<uint32_t>(p / out_width);
    const uint32_t ox = static_cast<uint32_t>(p - static_cast<size_t>(oy) * out_width);
    const size_t in_plane = static_cast<size_t>(in_width) * in_height;
    const uint32_t taps = kh * kw;
    const int32_t istride = static_cast<int32_t>(stride);
    float sum = bias ? bias[oc] : 0.0f;
    for (uint32_t ic = 0; ic < in_channels; ++ic) {
        const float * src = input + static_cast<size_t>(ic) * in_plane;
        const float * w = weight
            + (static_cast<size_t>(ic) * out_channels + oc) * taps;
        for (uint32_t ky = 0; ky < kh; ++ky) {
            const int32_t sy = static_cast<int32_t>(oy) + static_cast<int32_t>(pad)
                - static_cast<int32_t>(ky);
            if (sy < 0 || (sy % istride) != 0) continue;
            const int32_t iy = sy / istride;
            if (iy >= static_cast<int32_t>(in_height)) continue;
            for (uint32_t kx = 0; kx < kw; ++kx) {
                const int32_t sx = static_cast<int32_t>(ox) + static_cast<int32_t>(pad)
                    - static_cast<int32_t>(kx);
                if (sx < 0 || (sx % istride) != 0) continue;
                const int32_t ix = sx / istride;
                if (ix >= static_cast<int32_t>(in_width)) continue;
                sum += src[static_cast<size_t>(iy) * in_width + ix] * w[ky * kw + kx];
            }
        }
    }
    output[i] = sum;
}

extern "C" cudaError_t makepad_cuda_rife_conv_transpose2d_f32(
        const float * input,
        const float * weight,
        const float * bias,
        float * output,
        uint32_t in_width,
        uint32_t in_height,
        uint32_t out_width,
        uint32_t out_height,
        uint32_t in_channels,
        uint32_t out_channels,
        uint32_t kw,
        uint32_t kh,
        uint32_t pad,
        uint32_t stride,
        cudaStream_t stream) {
    if (!in_width || !in_height || !out_width || !out_height || !in_channels
            || !out_channels || !kw || !kh || !stride) {
        return cudaErrorInvalidValue;
    }
    const size_t n = static_cast<size_t>(out_width) * out_height * out_channels;
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<unsigned int>((n + block.x - 1) / block.x), 1, 1);
    makepad_cuda_rife_conv_transpose2d_f32_kernel<<<grid, block, 0, stream>>>(
        input, weight, bias, output, in_width, in_height, out_width, out_height,
        in_channels, out_channels, kw, kh, pad, stride);
    return cudaGetLastError();
}

// ResConv epilogue: out = LeakyReLU(conv * beta[c] + residual, slope).
static __global__ void makepad_cuda_rife_res_conv_f32_kernel(
        const float * __restrict__ conv,
        const float * __restrict__ residual,
        const float * __restrict__ beta,
        float * __restrict__ output,
        size_t plane,
        size_t n,
        float slope) {
    const size_t i = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    if (i >= n) return;
    const size_t channel = i / plane;
    const float value = conv[i] * beta[channel] + residual[i];
    output[i] = value < 0.0f ? value * slope : value;
}

extern "C" cudaError_t makepad_cuda_rife_res_conv_f32(
        const float * conv,
        const float * residual,
        const float * beta,
        float * output,
        size_t plane,
        size_t n,
        float slope,
        cudaStream_t stream) {
    if (!plane || !n) return cudaErrorInvalidValue;
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<unsigned int>((n + block.x - 1) / block.x), 1, 1);
    makepad_cuda_rife_res_conv_f32_kernel<<<grid, block, 0, stream>>>(
        conv, residual, beta, output, plane, n, slope);
    return cudaGetLastError();
}

// out = input * scale, and a plane fill: the flow rides the coarse-to-fine
// ladder scaled by each block's own factor (down on the way in, up on the
// way out), and the timestep enters as a constant plane.
static __global__ void makepad_cuda_rife_scale_f32_kernel(
        const float * __restrict__ input,
        float * __restrict__ output,
        size_t n,
        float scale) {
    const size_t i = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    if (i < n) output[i] = input[i] * scale;
}

extern "C" cudaError_t makepad_cuda_rife_scale_f32(
        const float * input,
        float * output,
        size_t n,
        float scale,
        cudaStream_t stream) {
    if (!n) return cudaErrorInvalidValue;
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<unsigned int>((n + block.x - 1) / block.x), 1, 1);
    makepad_cuda_rife_scale_f32_kernel<<<grid, block, 0, stream>>>(
        input, output, n, scale);
    return cudaGetLastError();
}

static __global__ void makepad_cuda_rife_fill_f32_kernel(
        float * __restrict__ output,
        size_t n,
        float value) {
    const size_t i = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    if (i < n) output[i] = value;
}

extern "C" cudaError_t makepad_cuda_rife_fill_f32(
        float * output,
        size_t n,
        float value,
        cudaStream_t stream) {
    if (!n) return cudaErrorInvalidValue;
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<unsigned int>((n + block.x - 1) / block.x), 1, 1);
    makepad_cuda_rife_fill_f32_kernel<<<grid, block, 0, stream>>>(output, n, value);
    return cudaGetLastError();
}

// The tail of the graph in one pass: sigmoid merge of the two warped frames,
// crop of the right/bottom padding, clamp, and the interleaved RGB8 the
// artifact wants. Only these bytes cross PCIe.
static __global__ void makepad_cuda_rife_merge_rgb8_f32_kernel(
        const float * __restrict__ warped0,
        const float * __restrict__ warped1,
        const float * __restrict__ mask,
        unsigned char * __restrict__ output,
        uint32_t padded_width,
        uint32_t padded_height,
        uint32_t width,
        uint32_t height) {
    const size_t out_plane = static_cast<size_t>(width) * height;
    const size_t i = static_cast<size_t>(blockIdx.x) * blockDim.x + threadIdx.x;
    if (i >= out_plane) return;
    const uint32_t y = static_cast<uint32_t>(i / width);
    const uint32_t x = static_cast<uint32_t>(i - static_cast<size_t>(y) * width);
    const size_t src = static_cast<size_t>(y) * padded_width + x;
    const size_t plane = static_cast<size_t>(padded_width) * padded_height;
    const float m = 1.0f / (1.0f + expf(-mask[src]));
    for (uint32_t c = 0; c < 3; ++c) {
        const float value = warped0[static_cast<size_t>(c) * plane + src] * m
            + warped1[static_cast<size_t>(c) * plane + src] * (1.0f - m);
        output[i * 3 + c] = static_cast<unsigned char>(
            roundf(fminf(fmaxf(value, 0.0f), 1.0f) * 255.0f));
    }
}

extern "C" cudaError_t makepad_cuda_rife_merge_rgb8_f32(
        const float * warped0,
        const float * warped1,
        const float * mask,
        unsigned char * output,
        uint32_t padded_width,
        uint32_t padded_height,
        uint32_t width,
        uint32_t height,
        cudaStream_t stream) {
    if (!padded_width || !padded_height || !width || !height
            || width > padded_width || height > padded_height) {
        return cudaErrorInvalidValue;
    }
    const size_t n = static_cast<size_t>(width) * height;
    const dim3 block(256, 1, 1);
    const dim3 grid(static_cast<unsigned int>((n + block.x - 1) / block.x), 1, 1);
    makepad_cuda_rife_merge_rgb8_f32_kernel<<<grid, block, 0, stream>>>(
        warped0, warped1, mask, output, padded_width, padded_height, width, height);
    return cudaGetLastError();
}
