import { init as initWebgl } from "./webgl.js";
import { sierpinski as sierpinskiJs } from "./sierpinski.js";

export async function main() {
  let now = performance.now();
  let vertices = sierpinskiJs(8);
  console.log("elapsed:", performance.now() - now);
  let canvas = document.querySelector("canvas");
  let gl = canvas.getContext("webgl");
  let { render } = initWebgl(gl, vertices);
  requestAnimationFrame(function frame(timestamp) {
    render(timestamp);
    requestAnimationFrame(frame);
  });
}

main();
