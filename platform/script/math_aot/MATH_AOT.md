# The splash math-AOT compiler: one IR, multiple backends
(2026-08-27, user: "architect this AOT math compiler with multiple
backends for actual JIT'ed assembly code (ARM/NEON and X86/SSE)").
The two tiers, in the user's words: an INTERPRETED AOT with stitch, and
a CODEGENNED AOT — real machine code — for max eval perf.

## Layering

```
splash source ──parser──► splash bytecode (unchanged, the interpreter's)
                              │  pure-math subset detector (conservative)
                              ▼
                            VIR — the vector IR
                              │
        ┌───────────────┬─────┴─────────┬───────────────┐
        ▼               ▼               ▼               ▼
   InterpBackend   StitchBackend   Arm64Backend    X64Backend
   (reference:     (wasm + spec    (codegen: NEON, (codegen: SSE2/4,
   the splash      v128 SIMD +     macOS MAP_JIT   Linux/Windows
   interpreter)    trig opcodes)   W^X)            fleet boxes)
```

**VIR** is a small typed linear IR (SSA-ish, no loops except the implicit
batch loop): values are `f32` or `f32x4` (vec2/vec3 ride f32x4 with
deterministically-zeroed spare lanes; mat3/mat4 are 3/4 f32x4 columns).
Ops are exactly the taught ISA: per-lane arith, neg, min/max/clamp/mix,
compare+select, dot/cross/normalize/length, mat*vec, mat*mat, splat,
lane extract/insert, shuffle, and the trig family (sin cos tan asin acos
atan atan2 exp ln pow sqrt abs). Nothing else — no memory ops (the
compiler alone emits the batch load/store), no calls, no branches beyond
select. That is what makes codegenning untrusted expressions memory-safe by
construction: the program cannot express an address.

## The backend contract

```rust
trait MathBackend {
    fn compile(&self, f: &VirFn) -> Result<Box<dyn CompiledMath>, BackendUnsupported>;
}
trait CompiledMath: Send + Sync {
    /// xyz: packed input points; out: one f32 per point (or per out arity).
    fn eval_batch(&self, xyz: &[f32], out: &mut [f32]);
}
```

Selection at runtime: native codegen for the host arch when available, else
StitchBackend — which is also the answer when a platform forbids executable
pages. InterpBackend is the semantic reference and the fallback for
anything the subset detector rejects (then it is not AOT at all — the
ordinary interpreter runs).

## Codegen mechanics

- Executable memory: `mmap` RW → RX with strict W^X; on macOS
  `MAP_JIT` + `pthread_jit_write_protect_np` around emission (Apple
  Silicon hardened runtime requires it). Page owner frees on drop.
- Register allocation: linear scan over VIR v-regs. Expression trees are
  shallow; NEON has 32 q-registers and SSE 16 xmm — spills are the rare
  case, to a fixed stack frame.
- ABI: `extern "C" fn(xyz: *const f32, out: *mut f32, n: usize)` — the
  batch loop is emitted by the backend, unrolled ×2 where it pays.
- Encodings are hand-emitted (no external assembler dependency), with
  disassembly SNAPSHOT tests: a corpus of tiny fixed expressions must
  produce byte-exact expected instruction sequences per backend.

## Trig in codegen land

Scalar libm calls per lane would destroy the SIMD win. The codegen backends
use vectorized minimax polynomial kernels (range-reduced sin/cos/tan,
atan2, exp/ln, pow = exp(ln·y)) emitted inline, 4 lanes at once, with a
documented accuracy contract (≤ 4 ULP vs libm, asserted by tests).

**Determinism note:** backends may differ in the last ULPs. This is NOT
load-bearing for gameplay: a model is meshed ONCE by its creator and
ships as the stored GLB (LOCALGEN law: the server only relays and
stores) — no two machines ever need to reproduce the same mesh.
StitchBackend stays the cross-platform bit-reference; the differential
suite holds JIT backends to the ULP contract against it.

## Testing (the path-localizing suite extends, per backend)

1. Per-op golden vectors (incl. NaN/inf/-0.0/denormals) run against
   EVERY backend — a wrong lane names one op in one backend.
2. Differential fuzz: thousands of random subset expressions,
   interpreter vs each backend, bit-identical for stitch, ULP-bounded
   for codegen; committed seed set + one recorded large run.
3. IR-level slot/lifetime battery (the stitch stack-block hazards live
   on as register-pressure tests for the codegen backends).
4. Batch edges: n = 0/1/non-multiple-of-unroll, unaligned buffers,
   interleaved compiled functions, page-permission smoke (write after
   RX must fault in debug harness).
5. Disassembly snapshots per backend; the sphere-mesh golden through
   csg_sdf on whichever backend the host selects.

## Rollout

1. Extract VIR + the backend trait from the in-flight stitch work (the
   current bytecode→stitch compiler splits into bytecode→VIR and
   VIR→stitch; its tests keep passing unchanged).
2. Arm64Backend (NEON) — the dev machines are Apple Silicon; this is
   where sculpt-cadence sampling runs first.
3. X64Backend (SSE2 baseline, SSE4.1 where detected) — the Windows/Linux
   fleet boxes.
4. AVX2/wider batching, only if the bench says the fleet needs it.

## Status (2026-08-27)

Rollout step 1 is LANDED: VIR + `MathBackend`/`CompiledMath` +
StitchBackend + VirInterpBackend (the VIR reference evaluator), with the
splash bytecode→VIR subset detector, uniform parameters (`Uniform` op +
`eval_batch(input, uniforms, out)`), the csg_sdf `SdfSplashExpr` adapter,
and the full path-localizing suite (per-op / spec-vector / slot-battery /
translate / differential-fuzz / batch-edge / mesh-golden layers) green.
One extension over the sketch above: VIR keeps f64 scalars with explicit
demote/promote, because splash scalar arithmetic is f64 and StitchBackend
is pinned bit-identical to the interpreter (codegen backends may fuse f64
pairs under their ULP contract). Numbers and details:
local/agent_state/delegate/mathaot-report.md. Next: Arm64Backend (NEON)
on this VIR.

## Audio shaders (2026-09-29)

The same "one IR, many backends, bit-reference tests" plan, grown with
state, memory, integers and structured control flow, lives in
`platform/script/audio_aot` (plan of record:
local/agent_state/edits/design/AUDIO-SHADERS.md). It has its own IR (AIR)
rather than extending VIR: VIR stays branchless and pinned bit-exact to the
Splash interpreter for SDFs, while audio needs loops, state and memory. Its
ARM64 backend is the first native codegen of this family; the encoder
patterns (hand-emitted words, MAP_JIT, linear scan over structured code)
carry over to VIR's planned Arm64Backend.
