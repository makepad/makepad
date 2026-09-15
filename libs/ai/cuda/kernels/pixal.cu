// Native sparse Pixal3D feature conditioning. See the trellis model crate's
// THIRD_PARTY_NOTICES.md. No NATTEN, unfold, or dense upsampled V allocation.
#include <cuda_runtime.h>
#include <stdint.h>
#include <float.h>
#include <math.h>

static __global__ void pixal_naf_sample_kernel(
    const float* __restrict__ q, const float* __restrict__ k,
    const float* __restrict__ v, const float* __restrict__ uv,
    float* __restrict__ out, uint32_t width, uint32_t height,
    uint32_t low_width, uint32_t low_height, uint32_t heads,
    uint32_t qc, uint32_t vc, uint32_t kernel) {
    const uint32_t token = blockIdx.x, head = blockIdx.y;
    const uint32_t qd = qc / heads, vd = vc / heads;
    const uint32_t kk = kernel * kernel;
    const int dx = width / low_width, dy = height / low_height;
    const int radius = kernel / 2;
    const float x = fminf(fmaxf(uv[2*token] * width - 0.5f, 0.0f), float(width-1));
    const float y = fminf(fmaxf(uv[2*token+1] * height - 0.5f, 0.0f), float(height-1));
    const int x0 = int(floorf(x)), y0 = int(floorf(y));
    const float tx = x-x0, ty = y-y0;
    // Up to 15x15; a single block owns one voxel/head and four sample pixels.
    extern __shared__ float scratch[];
    float* scores = scratch;
    int* indices = reinterpret_cast<int*>(scores + 4*kk);
    for (uint32_t i = threadIdx.x; i < 4*kk; i += blockDim.x) {
        const uint32_t corner = i / kk, tap = i % kk;
        const int px = min(x0 + int(corner & 1), int(width)-1);
        const int py = min(y0 + int(corner >> 1), int(height)-1);
        const int nx = px + (int(tap % kernel)-radius)*dx;
        const int ny = py + (int(tap / kernel)-radius)*dy;
        const bool valid = nx >= 0 && ny >= 0 && nx < int(width) && ny < int(height);
        const int p = valid ? int((ny/dy)*low_width + nx/dx) : -1;
        indices[i] = p;
        float sum = 0.0f;
        if (valid) {
            const size_t qi = (size_t(py)*width+px)*qc + head*qd;
            const size_t ki = size_t(p)*qc + head*qd;
            for (uint32_t c=0; c<qd; ++c) sum += q[qi+c]*k[ki+c];
        }
        scores[i] = sum * rsqrtf(float(qd));
    }
    __syncthreads();
    if (threadIdx.x < 4) {
        const uint32_t corner = threadIdx.x;
        float* s = scores + corner*kk;
        float maximum = -FLT_MAX;
        for (uint32_t i=0; i<kk; ++i) maximum = fmaxf(maximum,s[i]);
        float sum=0.0f;
        for (uint32_t i=0; i<kk; ++i) { s[i]=expf(s[i]-maximum); sum+=s[i]; }
        const float weight = ((corner&1)?tx:1.0f-tx)*((corner&2)?ty:1.0f-ty);
        for (uint32_t i=0; i<kk; ++i) s[i] *= weight/sum;
    }
    __syncthreads();
    for (uint32_t c=threadIdx.x; c<vd; c+=blockDim.x) {
        float value=0.0f;
        for (uint32_t i=0; i<4*kk; ++i) {
            const int p=indices[i];
            if (p>=0) value += scores[i]*v[size_t(p)*vc+head*vd+c];
        }
        out[size_t(token)*vc+head*vd+c]=value;
    }
}

extern "C" cudaError_t makepad_cuda_pixal_naf_sample_f32(
    const float* q, const float* k, const float* v, const float* uv, float* out,
    uint32_t count, uint32_t width, uint32_t height,
    uint32_t low_width, uint32_t low_height, uint32_t heads,
    uint32_t qc, uint32_t vc, uint32_t kernel, cudaStream_t stream) {
    if (!count) return cudaSuccess;
    const size_t shared = 4*kernel*kernel*(sizeof(float)+sizeof(int));
    pixal_naf_sample_kernel<<<dim3(count,heads),256,shared,stream>>>(
        q,k,v,uv,out,width,height,low_width,low_height,heads,qc,vc,kernel);
    return cudaGetLastError();
}

// Planar adaptive average pooling. Pool before transposing and applying
// positional embeddings; all spatial reductions remain on device.
static __global__ void pixal_pool_kernel(const float* x, float* out,
    uint32_t width, uint32_t height, uint32_t ow, uint32_t oh, uint32_t channels) {
    const size_t i = size_t(blockIdx.x)*blockDim.x+threadIdx.x;
    const size_t plane = size_t(ow)*oh;
    if (i >= plane*channels) return;
    const uint32_t c=i/plane, px=i%ow, py=(i%plane)/ow;
    const uint32_t x0=px*width/ow, x1=((px+1)*width+ow-1)/ow;
    const uint32_t y0=py*height/oh, y1=((py+1)*height+oh-1)/oh;
    float sum=0.0f;
    for(uint32_t y=y0;y<y1;++y) for(uint32_t xx=x0;xx<x1;++xx)
        sum+=x[(size_t(c)*height+y)*width+xx];
    out[i]=sum/float((x1-x0)*(y1-y0));
}

extern "C" cudaError_t makepad_cuda_pixal_pool_f32(const float* x, float* out,
    uint32_t width, uint32_t height, uint32_t ow, uint32_t oh, uint32_t channels,
    cudaStream_t stream) {
    const size_t n=size_t(ow)*oh*channels;
    if(!n) return cudaSuccess;
    pixal_pool_kernel<<<(n+255)/256,256,0,stream>>>(x,out,width,height,ow,oh,channels);
    return cudaGetLastError();
}

// Pooling produces planar 256-channel features. Fuse their transpose and
// four-head 2D RoPE, computing each phase once per pixel/pair on the GPU.
// This replaces two 128 MiB host phase tables at the 1024 guide resolution.
static __global__ void pixal_rope_kernel(const float* x, const float* periods,
    float* out, uint32_t width, uint32_t height) {
    const size_t i=size_t(blockIdx.x)*blockDim.x+threadIdx.x;
    const size_t pixels=size_t(width)*height;
    if(i>=pixels*32) return;
    const size_t pixel=i/32;
    const uint32_t pair=i%32;
    const float coord=pair<16 ? float(pixel/width) : float(pixel%width);
    const float side=pair<16 ? float(height) : float(width);
    const float p=2.0f*(coord+0.5f)/side-1.0f;
    float sine,cosine;
    __sincosf(6.283185307179586f*p/periods[pair%16],&sine,&cosine);
    for(uint32_t head=0;head<4;++head) {
        const uint32_t channel=head*64+pair;
        const float a=x[size_t(channel)*pixels+pixel];
        const float b=x[size_t(channel+32)*pixels+pixel];
        out[pixel*256+channel]=a*cosine-b*sine;
        out[pixel*256+channel+32]=b*cosine+a*sine;
    }
}

extern "C" cudaError_t makepad_cuda_pixal_rope_f32(const float* x, const float* periods,
    float* out, uint32_t width, uint32_t height, cudaStream_t stream) {
    const size_t count=size_t(width)*height*32;
    if(!count) return cudaSuccess;
    pixal_rope_kernel<<<(count+255)/256,256,0,stream>>>(x,periods,out,width,height);
    return cudaGetLastError();
}
