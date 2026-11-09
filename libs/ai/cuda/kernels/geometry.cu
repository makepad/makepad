// Deterministic plane-based multi-view PatchMatch. No texture filtering.
#include <cuda_runtime.h>
#include <stdint.h>
#include <math_constants.h>

struct GeometryCamera {
    float k[4], rotation[9], translation[3];
    uint32_t width, height;
    uint64_t offset;
};
struct GeometryParams {
    uint64_t seed;
    uint32_t width, height, radius, step, top_k, sources;
    float sigma_spatial, sigma_color, depth_min, depth_max;
};
struct GeometrySweep { uint64_t iteration; uint32_t colour; float scale; };
struct Hypothesis { float depth; float3 normal; float cost; };
struct Pixel { int x, y; uint64_t i; };
struct Context {
    const float *images;
    const GeometryCamera *cameras;
    GeometryParams p;
};

static __device__ float dot(float3 a, float3 b) {
    return a.x*b.x + a.y*b.y + a.z*b.z;
}
static __device__ float3 normalise(float3 a) {
    float length = sqrtf(dot(a,a));
    return make_float3(a.x/length, a.y/length, a.z/length);
}
static __device__ float3 ray(const GeometryCamera &c, int x, int y) {
    return make_float3((float(x)-c.k[2])/c.k[0], (float(y)-c.k[3])/c.k[1], 1.0f);
}
static __device__ float draw(uint64_t seed, uint64_t iteration, uint64_t pixel, uint64_t slot) {
    uint64_t z = seed ^ (iteration << 48) ^ (slot << 40) ^ pixel;
    z += UINT64_C(0x9e3779b97f4a7c15);
    z = (z ^ (z >> 30)) * UINT64_C(0xbf58476d1ce4e5b9);
    z = (z ^ (z >> 27)) * UINT64_C(0x94d049bb133111eb);
    z ^= z >> 31;
    return float(2.0 * double(z >> 11) / 9007199254740992.0 - 1.0);
}
static __device__ float random_depth(const GeometryParams &p, float u) {
    float low = 1.0f/p.depth_max;
    return 1.0f/(low + (1.0f/p.depth_min-low)*(u+1.0f)*0.5f);
}
static __device__ float3 random_normal(float3 direction, float u1, float u2) {
    float3 axis = normalise(direction);
    axis = make_float3(-axis.x,-axis.y,-axis.z);
    float3 tangent = normalise(make_float3(axis.z,0.0f,-axis.x));
    float3 bitangent = make_float3(axis.y*tangent.z-axis.z*tangent.y,
        axis.z*tangent.x-axis.x*tangent.z, axis.x*tangent.y-axis.y*tangent.x);
    float cosine = (1.0f-u1)*0.5f;
    float azimuth = CUDART_PI_F*(u2+1.0f);
    float sine = sqrtf(1.0f-cosine*cosine);
    float ca = cosf(azimuth), sa = sinf(azimuth);
    return normalise(make_float3(axis.x*cosine+sine*(tangent.x*ca+bitangent.x*sa),
        axis.y*cosine+sine*(tangent.y*ca+bitangent.y*sa),
        axis.z*cosine+sine*(tangent.z*ca+bitangent.z*sa)));
}
static __device__ bool region(const Context &c, int64_t x, int64_t y) {
    return x >= c.p.radius && y >= c.p.radius
        && x < int64_t(c.cameras[0].width)-c.p.radius
        && y < int64_t(c.cameras[0].height)-c.p.radius;
}
static __device__ bool sample(const float *image, const GeometryCamera &c,
        float x, float y, float &value) {
    if (!isfinite(x) || !isfinite(y) || x < 0.0f || y < 0.0f
        || x > float(c.width-1) || y > float(c.height-1)) return false;
    // At large dimensions the f32 rectangle endpoint can round upward.
    uint32_t x0 = min(uint32_t(floorf(x)), c.width-1);
    uint32_t y0 = min(uint32_t(floorf(y)), c.height-1);
    uint32_t x1 = min(x0+1,c.width-1), y1 = min(y0+1,c.height-1);
    float tx = x-float(x0), ty = y-float(y0);
    float a = image[uint64_t(y0)*c.width+x0]*(1.0f-tx)+image[uint64_t(y0)*c.width+x1]*tx;
    float b = image[uint64_t(y1)*c.width+x0]*(1.0f-tx)+image[uint64_t(y1)*c.width+x1]*tx;
    value = a*(1.0f-ty)+b*ty;
    return true;
}
static __device__ float ncc(const Context &c, const GeometryCamera &s,
        const float h[9], Pixel pixel) {
    const GeometryCamera &r = c.cameras[0];
    const float *ref = c.images+r.offset, *src = c.images+s.offset;
    float centre = ref[pixel.i];
    float sw=0.0f, sr=0.0f, ss=0.0f, srr=0.0f, sss=0.0f, srs=0.0f, first=0.0f;
    // u64 loop counters also handle a step larger than the window span.
    uint64_t span = uint64_t(c.p.radius)*2;
    for (uint64_t oy=0; oy<=span; oy+=c.p.step) {
        for (uint64_t ox=0; ox<=span; ox+=c.p.step) {
            float dx=float(ox)-float(c.p.radius), dy=float(oy)-float(c.p.radius);
            float qx=float(pixel.x)+dx, qy=float(pixel.y)+dy;
            float rv, sv;
            if (!sample(ref,r,qx,qy,rv)) return 2.0f;
            float w=h[6]*qx+h[7]*qy+h[8];
            if (!isfinite(w) || w<=0.0f) return 2.0f;
            float sx=(h[0]*qx+h[1]*qy+h[2])/w;
            float sy=(h[3]*qx+h[4]*qy+h[5])/w;
            if (!sample(src,s,sx,sy,sv)) return 2.0f;
            if (ox==0 && oy==0) first=sv;
            rv-=centre;
            sv-=first;
            float wx=dx/c.p.sigma_spatial, wy=dy/c.p.sigma_spatial, wc=rv/c.p.sigma_color;
            float weight=expf(-0.5f*(wx*wx+wy*wy+wc*wc));
            sw+=weight; sr+=weight*rv; ss+=weight*sv;
            srr+=weight*rv*rv; sss+=weight*sv*sv; srs+=weight*rv*sv;
        }
    }
    float mr=sr/sw, ms=ss/sw;
    float vr=srr/sw-mr*mr, vs=sss/sw-ms*ms;
    if (!isfinite(vr) || !isfinite(vs) || vr<1e-6f || vs<1e-6f) return 2.0f;
    float covariance=srs/sw-mr*ms;
    return fminf(2.0f,fmaxf(0.0f,1.0f-covariance/sqrtf(vr*vs)));
}
static __device__ float cost(const Context &c, Pixel pixel, float depth, float3 normal) {
    const GeometryCamera &r = c.cameras[0];
    float nd = dot(normal,ray(r,pixel.x,pixel.y));
    if (!isfinite(depth) || depth<=0.0f || !isfinite(normal.x) || !isfinite(normal.y)
        || !isfinite(normal.z) || nd>=0.0f) return 2.0f;
    float delta=-depth*nd, n[3]={normal.x,normal.y,normal.z};
    float costs[16];
    unsigned valid=0;
    for (unsigned source=1; source<=c.p.sources; ++source) {
        const GeometryCamera &s = c.cameras[source];
        float h[9];
        for (int i=0;i<3;++i) {
            for (int j=0;j<3;++j) h[i*3+j]=s.rotation[i*3+j]-s.translation[i]*n[j]/delta;
            h[i*3]/=r.k[0]; h[i*3+1]/=r.k[1];
            h[i*3+2]-=h[i*3]*r.k[2]+h[i*3+1]*r.k[3];
        }
        for (int j=0;j<3;++j) {
            h[j]=s.k[0]*h[j]+s.k[2]*h[6+j];
            h[3+j]=s.k[1]*h[3+j]+s.k[3]*h[6+j];
        }
        float v=ncc(c,s,h,pixel);
        if (v<2.0f) {
            unsigned j=valid++;
            while (j>0 && v<costs[j-1]) { costs[j]=costs[j-1]; --j; }
            costs[j]=v;
        }
    }
    unsigned count=min(valid,c.p.top_k);
    if (!count) return 2.0f;
    float sum=0.0f;
    for (unsigned j=0;j<count;++j) sum+=costs[j];
    return sum/float(count);
}
static __device__ Hypothesis load(const float *map, uint64_t i) {
    const float *p=map+i*5;
    return {p[0],make_float3(p[1],p[2],p[3]),p[4]};
}
static __device__ void save(float *map, uint64_t i, Hypothesis h) {
    float *p=map+i*5;
    p[0]=h.depth; p[1]=h.normal.x; p[2]=h.normal.y; p[3]=h.normal.z; p[4]=h.cost;
}
static __device__ void consider(const Context &c, Pixel pixel, Hypothesis &best, float d, float3 n) {
    float candidate=cost(c,pixel,d,n);
    if (candidate<best.cost) best={d,n,candidate};
}
static __device__ void init_pixel(Context c, float *map, unsigned initial, uint64_t i) {
    const GeometryCamera &r=c.cameras[0];
    if (i>=uint64_t(r.width)*r.height) return;
    Pixel pixel={int(i%r.width),int(i/r.width),i};
    Hypothesis h;
    if (initial) h=load(map,i);
    else h={random_depth(c.p,draw(c.p.seed,0xffff,i,0)),
        random_normal(ray(r,pixel.x,pixel.y),draw(c.p.seed,0xffff,i,1),draw(c.p.seed,0xffff,i,2)),2.0f};
    h.cost=region(c,pixel.x,pixel.y)?cost(c,pixel,h.depth,h.normal):2.0f;
    save(map,i,h);
}
static __device__ void sweep_pixel(Context c, float *map, GeometrySweep sweep, uint64_t index) {
    const GeometryCamera &r=c.cameras[0];
    uint64_t half_width=(uint64_t(r.width)+1)/2;
    int64_t y=index/half_width;
    int64_t x=2*(index%half_width)+((sweep.colour+uint64_t(y))%2);
    if (!region(c,x,y)) return;
    Pixel pixel={int(x),int(y),uint64_t(y)*r.width+uint64_t(x)};
    Hypothesis best=load(map,pixel.i);
    const int dx[8]={0,0,-1,1,0,0,-5,5}, dy[8]={-1,1,0,0,-5,5,0,0};
    for (int k=0;k<8;++k) {
        int64_t nx=x+dx[k], ny=y+dy[k];
        if (!region(c,nx,ny)) continue;
        Hypothesis neighbour=load(map,uint64_t(ny)*r.width+uint64_t(nx));
        consider(c,pixel,best,neighbour.depth,neighbour.normal);
    }
    // Every refinement is relative to the propagation winner, even after a win.
    float d=best.depth;
    float3 n=best.normal;
    float inverse=1.0f/d+sweep.scale*0.25f*(1.0f/c.p.depth_min-1.0f/c.p.depth_max)
        *draw(c.p.seed,sweep.iteration,pixel.i,0);
    inverse=fminf(1.0f/c.p.depth_min,fmaxf(1.0f/c.p.depth_max,inverse));
    float dp=1.0f/inverse;
    float3 np=make_float3(n.x+sweep.scale*0.5f*draw(c.p.seed,sweep.iteration,pixel.i,1),
        n.y+sweep.scale*0.5f*draw(c.p.seed,sweep.iteration,pixel.i,2),
        n.z+sweep.scale*0.5f*draw(c.p.seed,sweep.iteration,pixel.i,3));
    float3 direction=ray(r,pixel.x,pixel.y);
    if (dot(np,np)>0.0f) {
        np=normalise(np);
        if (dot(np,direction)>=0.0f) np=make_float3(-np.x,-np.y,-np.z);
    } else np=n;
    float dr=random_depth(c.p,draw(c.p.seed,sweep.iteration,pixel.i,4));
    float3 nr=random_normal(direction,draw(c.p.seed,sweep.iteration,pixel.i,5),
        draw(c.p.seed,sweep.iteration,pixel.i,6));
    consider(c,pixel,best,dp,n);
    consider(c,pixel,best,d,np);
    consider(c,pixel,best,dp,np);
    consider(c,pixel,best,dr,n);
    consider(c,pixel,best,d,nr);
    save(map,pixel.i,best);
}

static __global__ void geometry_init(Context c, float *map, unsigned initial) {
    uint64_t count=uint64_t(c.p.width)*c.p.height;
    for (uint64_t i=uint64_t(blockIdx.x)*blockDim.x+threadIdx.x; i<count;
            i+=uint64_t(gridDim.x)*blockDim.x) init_pixel(c,map,initial,i);
}
static __global__ void geometry_sweep(Context c, float *map, GeometrySweep sweep) {
    uint64_t count=((uint64_t(c.p.width)+1)/2)*c.p.height;
    for (uint64_t i=uint64_t(blockIdx.x)*blockDim.x+threadIdx.x; i<count;
            i+=uint64_t(gridDim.x)*blockDim.x) sweep_pixel(c,map,sweep,i);
}
extern "C" cudaError_t makepad_cuda_geometry_init(const float *images,
        const GeometryCamera *cameras, GeometryParams params, float *map,
        uint32_t initial, cudaStream_t stream) {
    uint64_t blocks=(uint64_t(params.width)*params.height+127)/128;
    unsigned grid=unsigned(blocks<65535?blocks:65535);
    geometry_init<<<grid,128,0,stream>>>({images,cameras,params},map,initial);
    return cudaGetLastError();
}
extern "C" cudaError_t makepad_cuda_geometry_sweep(const float *images,
        const GeometryCamera *cameras, GeometryParams params, float *map,
        GeometrySweep sweep, cudaStream_t stream) {
    uint64_t blocks=(((uint64_t(params.width)+1)/2)*params.height+127)/128;
    unsigned grid=unsigned(blocks<65535?blocks:65535);
    geometry_sweep<<<grid,128,0,stream>>>({images,cameras,params},map,sweep);
    return cudaGetLastError();
}
