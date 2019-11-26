import { init as initWebgl } from "./webgl.js";
import { sierpinski as sierpinskiJs } from "./sierpinski.js";

export async function main() {
  let now = Date.now();
  let vertices = sierpinskiJs(8);
  console.log(Date.now() - now);
  let canvas = document.getElementById("canvas");
  let gl = canvas.getContext("webgl");
  let { render } = initWebgl(gl, vertices);
  requestAnimationFrame(function frame() {
    render();
    requestAnimationFrame(frame);
  });
}

main();
