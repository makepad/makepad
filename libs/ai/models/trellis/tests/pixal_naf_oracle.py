"""Validate the native CUDA NAF sampler against dense PyTorch operations.

Build kernels/pixal.cu with nvcc -shared -O3, then pass the resulting library.
This is an offline test oracle; production inference never imports Python.
"""
import argparse
import ctypes
import json
import time

import torch
import torch.nn.functional as F


def dense_reference(q, k, v, width, low_width, heads, kernel):
    qc, vc = q.shape[-1], v.shape[-1]
    qd, vd = qc // heads, vc // heads
    dilation = width // low_width
    q = q.reshape(width, width, heads, qd).permute(2, 0, 1, 3).reshape(heads, width*width, qd)
    def unfold(x, channels):
        x = x.reshape(low_width, low_width, heads, channels).permute(2, 3, 0, 1)
        x = F.interpolate(x, size=(width, width), mode="nearest-exact")
        x = F.unfold(x, kernel, dilation=dilation, padding=(kernel//2)*dilation)
        return x.reshape(heads, channels, kernel*kernel, width*width).permute(0, 3, 2, 1)
    keys = unfold(k, qd)
    scores = (q.unsqueeze(2)*keys).sum(-1) * qd**-0.5
    attn = scores.softmax(-1)
    del keys, scores
    chunks = []
    # Bound the oracle's unfold workspace without changing the math.
    values = v.reshape(low_width*low_width, heads, vd)
    for start in range(0, vd, 16):
        part = values[:, :, start:start+16].reshape(low_width*low_width, -1)
        val = unfold(part, part.shape[-1]//heads)
        chunks.append((attn.unsqueeze(-1)*val).sum(-2))
    out = torch.cat(chunks, dim=-1).permute(1,0,2).reshape(width,width,vc)
    return out.permute(2,0,1).unsqueeze(0)


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument("library")
    parser.add_argument("--benchmark", action="store_true")
    args=parser.parse_args()
    lib=ctypes.CDLL(args.library)
    op=lib.makepad_cuda_pixal_naf_sample_f32
    op.argtypes=[ctypes.c_void_p]*5+[ctypes.c_uint32]*9+[ctypes.c_void_p]
    op.restype=ctypes.c_int
    torch.manual_seed(19)
    torch.backends.cuda.matmul.allow_tf32=False
    def native(q,k,v,uv,width,low,heads,kernel):
        out=torch.empty((len(uv),v.shape[-1]),device="cuda",dtype=torch.float32)
        status=op(q.data_ptr(),k.data_ptr(),v.data_ptr(),uv.data_ptr(),out.data_ptr(),
            len(uv),width,width,low,low,heads,q.shape[-1],v.shape[-1],kernel,
            torch.cuda.current_stream().cuda_stream)
        if status: raise RuntimeError(f"CUDA launch status {status}")
        return out
    for width,low,qc,vc,heads,kernel in [(8,2,8,16,2,3),(32,4,256,1024,4,9),(48,12,32,64,4,5)]:
        q=torch.randn(width*width,qc,device="cuda")
        k=torch.randn(low*low,qc,device="cuda")
        v=torch.randn(low*low,vc,device="cuda")
        uv=torch.rand(73,2,device="cuda")*1.4-0.2
        uv[:4]=torch.tensor([[0,0],[1,1],[0.5,0.5],[-1,2]],device="cuda")
        dense=dense_reference(q,k,v,width,low,heads,kernel)
        expected=F.grid_sample(dense,(uv*2-1).view(1,-1,1,2),padding_mode="border",align_corners=False)
        expected=expected[0,:,:,0].T.contiguous()
        actual=native(q,k,v,uv,width,low,heads,kernel)
        torch.testing.assert_close(actual,expected,rtol=3e-5,atol=3e-6)
        print(json.dumps({"width":width,"max_error":float((actual-expected).abs().max())}),flush=True)
    if args.benchmark:
        for width,low,count in [(512,32,12000),(512,64,30000),(1024,64,30000)]:
            q=torch.randn(width*width,256,device="cuda")
            k=torch.randn(low*low,256,device="cuda")
            v=torch.randn(low*low,1024,device="cuda")
            uv=torch.rand(count,2,device="cuda")
            for _ in range(2): actual=native(q,k,v,uv,width,low,4,9)
            torch.cuda.synchronize()
            start=time.perf_counter()
            for _ in range(5): actual=native(q,k,v,uv,width,low,4,9)
            torch.cuda.synchronize()
            print(json.dumps({"width":width,"low":low,"voxels":count,
                "sample_ms":(time.perf_counter()-start)*200,
                "output_bytes":actual.numel()*4,"dense_output_bytes":width*width*1024*4}),flush=True)


if __name__=="__main__": main()
