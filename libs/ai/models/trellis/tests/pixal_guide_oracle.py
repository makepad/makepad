"""Compare pixal_naf_check output with checkpoint-driven PyTorch guide + NAF.

python pixal_guide_oracle.py dino_naf.safetensors OUTPUT.f32
Requires torch and safetensors. This is an offline oracle, not production code.
The reference uses ordinary conv2d, group_norm, adaptive_avg_pool2d, RoPE,
unfold/softmax and grid_sample; it does not call any Makepad CUDA kernels.
"""
import argparse
import json
import math
from pathlib import Path
import struct
import torch
import torch.nn.functional as F
from safetensors import safe_open
from pixal_naf_oracle import dense_reference


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("weights")
    parser.add_argument("native_output")
    args = parser.parse_args()
    torch.backends.cuda.matmul.allow_tf32 = False
    torch.backends.cudnn.allow_tf32 = False
    with safe_open(args.weights, framework="pt", device="cuda") as checkpoint:
        weights = {key: checkpoint.get_tensor(key).float() for key in checkpoint.keys() if key.startswith("naf.")}
    image = torch.tensor([(i*13)%257/256 for i in range(3*32*32)],device="cuda").reshape(1,3,32,32)
    branches = []
    for branch,kernel in [("encoder",1),("sem_encoder",3)]:
        prefix = f"naf.image_encoder.{branch}"
        def conv(x, suffix):
            if kernel == 3: x = F.pad(x,(1,1,1,1),mode="reflect")
            return F.conv2d(x,weights[f"{prefix}.{suffix}.weight"],weights[f"{prefix}.{suffix}.bias"])
        x = conv(image,"0")
        for block in [1,2]:
            for layer in [1,2]:
                norm = f"{prefix}.{block}.norm{layer}"
                x = F.group_norm(x,8,weights[f"{norm}.weight"],weights[f"{norm}.bias"],1e-5)
                x = conv(F.silu(x),f"{block}.conv{layer}")
        branches.append(x)
    encoded = torch.cat(branches,dim=1)
    values = torch.tensor([(i*17)%101/50-1 for i in range(16*1024)],device="cuda").reshape(16,1024)
    uv = [[0,0],[1,1],[0.5,0.5],[-1,2]]+[[i/28,(i*7)%29/28] for i in range(29)]
    uv = torch.tensor(uv,device="cuda")
    raw = Path(args.native_output).read_bytes()
    actual = torch.tensor(struct.unpack(f"<{len(raw)//4}f",raw),device="cuda").reshape(2,33,1024)
    for index,target in enumerate([16,32]):
        pooled = F.adaptive_avg_pool2d(encoded,(target,target))
        q = pooled.permute(0,2,3,1).reshape(target*target,4,64)
        coords = torch.arange(target,device="cuda",dtype=torch.float32).add(.5).div(target).mul(2).sub(1)
        coords = torch.stack(torch.meshgrid(coords,coords,indexing="ij"),dim=-1).reshape(-1,2)
        angles = (2*math.pi*coords[:,:,None]/weights["naf.image_encoder.rope.periods"]).flatten(1).repeat(1,2)[:,None,:]
        q = q*angles.cos()+torch.cat([-q[:,:,32:],q[:,:,:32]],dim=-1)*angles.sin()
        q = q.reshape(target*target,256)
        k = F.adaptive_avg_pool2d(q.T.reshape(1,256,target,target),(4,4)).reshape(256,16).T.contiguous()
        dense = dense_reference(q,k,values,target,4,4,9)
        expected = F.grid_sample(dense,(uv*2-1).reshape(1,-1,1,2),padding_mode="border",align_corners=False)[0,:,:,0].T
        error = (actual[index]-expected).abs()
        # Native convolutions use tensor-core F16 operands and F32 accumulation.
        torch.testing.assert_close(actual[index],expected,rtol=0.003,atol=0.001)
        print(json.dumps({"target":target,"max_error":float(error.max()),"mean_error":float(error.mean())}),flush=True)


if __name__ == "__main__": main()
