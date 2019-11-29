import { init as initWasm } from "./wasm.js";
import { init as initWebgl } from "./webgl.js";
// import { sierpinski as sierpinskiJs } from "./sierpinski.js";

export async function main() {
  let { sierpinski: sierpinskiWasm } = await initWasm(
    "/rust_workshop/target/wasm32-unknown-unknown/release/step_6_wasm.wasm"
  );
  let now = performance.now();
  let vertices = sierpinskiWasm(8);
  console.log(performance.now() - now);

  let canvas = document.getElementById("canvas");

  // Set the canvas' size based on device pixel ratio
  const { width, height } = getComputedStyle(canvas);
  canvas.width = parseFloat(width) * devicePixelRatio;
  canvas.height = parseFloat(height) * devicePixelRatio;

  let gl = canvas.getContext("webgl");
  let { render } = initWebgl(gl, vertices);

  requestAnimationFrame(function frame(timestamp) {
    render(timestamp);
    requestAnimationFrame(frame);
  });
}

main();
