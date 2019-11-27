# Step 1

* In the file `/step_1/src/lib.rs`, add the following lines:
  
      #![allow(dead_code)]

      extern "C" {
          fn alert(level: i32);
      }

      #[no_mangle]
      extern "C" fn sierpinski(level: i32) {
          unsafe {
              alert(level);
          }
      }

* In the file `/step_1/static/wasm.js`, add the following lines:

      export async function init(url) {
        let response = await fetch(url);
        let buffer = await response.arrayBuffer();
        let result = await WebAssembly.instantiate(buffer, {
            env: { alert }
        });
        return result.instance.exports;
      }

* In the file `/static/main.js`, add the following line:

      import { init as initWasm } from "./wasm.js";

* In the file `/step_1/static/main.js`:

  * Replace the following lines:

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

  * With:

        export async function main() {
          let { sierpinski: sierpinskiWasm } = await initWasm("/rust_workshop/target/wasm32-unknown-unknown/release/step_1_wasm.wasm"
          );
          sierpinskiWasm(8);
        }