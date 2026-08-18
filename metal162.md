# Metal on .162 — first SAM3 path

CUDA lanes are done. This branch is the Metal port. **No oracle.** Goal:
as fast as possible on the in-repo Metal ggml executor.

## Box

`10.0.0.162` M4 16 GB. Tunnel `:8384 --no-sync`. Workdir
`/Users/dev/metal-probe/sam3/`.

## What landed first

`libs/ggml/src/backend/metal/gpu_tensor.rs`: SAM3's `gpu_*` API now
runs on macOS. Heavy GEMM / flash / conv / LN / add/mul/silu/gelu go
through existing Metal `try_*` kernels. Addressing (slice/concat/gather/
RPB/sine/refine) stays on the host. CUDA graph capture fails closed and
SAM3 already falls back to eager.

This is **not** a fully device-resident graph yet. Copies around `try_*`
are the first thing to cut.

## Next

1. Release-build `sam3-validate` on .162, fetch Comfy-Org multiplex
   weights, time a warm detect+refine.
2. Keep activations in MTLBuffers (stop host roundtrips on try_*).
3. Then Klein (stage TE) and RealESRGAN Metal.
