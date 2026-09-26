# Chromium 63 GPU-process crash on `useProgram` with std140 uniform blocks

This is the crash report for the `chrome63_ubo_crash_repro.html` in this
directory, and the reason the accompanying patches are needed. It is a
**browser bug, not an application bug**: the minimal repro is ~40 lines of
plain WebGL2 with no makepad code in it.

## Symptom

The app boots, the first frames draw, and then the browser dies a few seconds
later. The console is **empty** — no exception, no `webglcontextlost` handler
output before the process goes — which makes it very easy to misattribute to the
app. Chrome's own stderr has the real story:

```
Received signal 11 SEGV_MAPERR 000000000000
#0 base::debug::StackTrace::StackTrace()
#6 gpu::gles2::Program::ClearUniforms()
#7 gpu::gles2::GLES2DecoderImpl::HandleUseProgram()
#8 gpu::GLES2DecoderImpl::DoCommandsImpl<>()
#9 gpu::CommandBufferService::Flush()
#10 gpu::GpuCommandBufferStub::OnAsyncFlush()
#13 gpu::GpuChannel::HandleMessageHelper()
#14 gpu::GpuChannel::HandleMessage()
```

A null-pointer dereference **inside Chromium's own GLES2 implementation**,
reached from `gl.useProgram()`. When the GPU process dies, the page receives
`webglcontextlost` and makepad correctly stops rendering and reports
`makepad: WebGL context lost; rendering stopped until an explicit reload`.

## Reproduction

Browser: Chromium **63.0.3220.0** (official `chromium:63` Docker image, or a
local 63 build). Reproduced both headless and windowed, on SwiftShader *and* on
a real driver via `--device /dev/dri`, so it is not a software-rasterizer
artifact.

```sh
# from the makepad checkout, serving this directory
python3 -m http.server 8095

docker run --rm --net=host --device /dev/dri \
  chromium:63 chromium \
  --no-sandbox --headless --ignore-gpu-blacklist \
  --dump-dom "http://127.0.0.1:8095/chrome63_ubo_crash_repro.html?ubo=1"
# -> GPU process dies in Program::ClearUniforms after a few useProgram calls
```

The `?ubo=` switch is the whole experiment. The two programs are identical
except for how the uniform is declared:

| query | uniform declaration | result |
|---|---|---|
| `?ubo=1` | `layout(std140) uniform draw_call { vec4 tint; vec4 pad0; vec4 pad1; vec4 pad2; };` | **GPU process SEGV** |
| `?ubo=0` | `uniform vec4 tint;` | all 8 `useProgram` calls succeed, no crash |

`?n=<k>` controls how many distinct programs are built (default 8). A single
block of four `vec4`s is enough; the block does not need to be large, and it
does not need to appear in both shader stages.

## Why makepad runs into it

`platform/script/src/shader_glsl.rs` emits std140 uniform blocks for every
draw shader:

- `passUniforms` — `draw_pass` IO → `DrawPassUniforms` (`platform/src/draw_pass.rs`)
- `draw_listUniforms` — `draw_list` IO → `DrawListUniforms`
- `draw_callUniforms` — `draw_call` IO → `DrawCallUniforms`
- `userUniforms` — for `ShaderIoKind::Uniform` IOs
- `liveUniforms` — for `ShaderIoKind::ScopeUniform` IOs
- `<name>_Uniforms` — one per app-declared `ShaderIoKind::UniformBuffer` IO

So a normal makepad draw shader carries at least three blocks, and the renderer
switches the current program between several such programs every frame. That
switching is what trips the fault. On the crash path the two programs that bound
successfully reported 58 and 27 active uniforms, all of them block members
(`unibuf_draw_call.*`, `unibuf_draw_pass.*`), and the third `useProgram` killed
the process.

Note the nesting the generator emits: a std140 block whose single member is a
*struct* instance (`unibuf_draw_call`) that then expands again. That is worth
ruling out as an aggravating factor when filing upstream.

## Impact and options

The app cannot render on Chromium 63 at all: the GPU process dies on the first
frames. wasm threads are unavailable there anyway (Chrome 74+), so the
`--no-threads` build is mandatory regardless.

1. **Do not emit std140 uniform blocks in the web backend** — the one change
   that avoids the faulty path, but it touches the renderer's uniform plumbing.
2. **Target a newer webview.** Given the Chrome 74 requirement for wasm threads,
   this is the cheaper route to a working TV app.
3. **File upstream against Chromium.** This directory is the self-contained
   test case.

## Not the cause (ruled out while diagnosing)

- Not the app: reproduces with a stock `makepad-example-counter` (one label, one
  button).
- Not SwiftShader: identical crash with a real driver via `--device /dev/dri`.
- Not memory budget: lowering the WebGL allocation limit did not change it.
- Not a missing top-level `await` / `import()`: those were real Chrome 63 parse
  blockers, fixed by the accompanying patches, and the crash happens after the
  app has already booted and drawn.
