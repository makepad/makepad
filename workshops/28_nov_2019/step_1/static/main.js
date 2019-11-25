import { init as initWasm } from "./wasm.js";
import { init as initWebgl } from "./webgl.js";
import { sierpinski as sierpinskiJs } from "./sierpinski.js";

export async function main() {
  let { sierpinski: sierpinskiWasm } = await initWasm(
    "/rust_workshop/target/wasm32-unknown-unknown/release/step_1_wasm.wasm"
  );
  let now = Date.now();
  let vertices = sierpinskiWasm(8);
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

function liveReloader() {
    var req = new XMLHttpRequest()
    req.timeout = 60000
    req.responseType = 'text'
    req.addEventListener("load", function() {
        if (req.status === 201) return liveReloader();
        if (req.status === 200) {
            var msg = JSON.parse(req.response);
            if (msg.type == "file_change") {
                location.href = location.href
            }
        }
    })
    req.open("GET", "/$watch?" + ('' + Math.random()).slice(2))
    req.send()
}
liveReloader()
