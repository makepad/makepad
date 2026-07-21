// Web runtime selector: WebGL2 only.

export async function createMakepadWebBackend(wasm, dispatch, canvas) {
  const web_gl = await import("./web_gl.js");
  const WasmWebGL = web_gl.WasmWebGL;

  console.log("[makepad] backend=webgl2");
  return new WasmWebGL(wasm, dispatch, canvas);
}
