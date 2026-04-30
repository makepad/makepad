// Web runtime selector: prefer WebGPU, fallback to WebGL2.

export async function createMakepadWebBackend(wasm, dispatch, canvas) {
  const [{ WasmWebGPU }, { WasmWebGL }] = await Promise.all([
    import("./web_gpu.js"),
    import("./web_gl.js"),
  ]);

  // Prefer WebGPU when available, but only if it supports Makepad’s current
  // render protocol. Today the wasm side still emits WebGL `FromWasm*` messages.
  const webgpu = await WasmWebGPU.try_create(wasm, dispatch, canvas);
  if (webgpu) {
    const supports_current_protocol =
      typeof webgpu.FromWasmRenderCommandBuffer === "function" ||
      typeof webgpu.FromWasmDrawCall === "function" ||
      typeof webgpu.FromWasmCompileWebGLShader === "function";
    if (supports_current_protocol) {
      console.log("[makepad] backend=webgpu");
      return webgpu;
    }
    console.warn(
      "[makepad] backend=webgpu unavailable for current protocol; falling back to webgl2",
    );
  }

  console.log("[makepad] backend=webgl2");
  return new WasmWebGL(wasm, dispatch, canvas);
}
