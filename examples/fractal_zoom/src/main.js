import {WasmWebGL} from "/makepad/platform/src/os/web_browser/web_gl.js";

const wasm = await WasmWebGL.fetch_and_instantiate_wasm(
    location.hostname=="127.0.0.1"?
    "/makepad/target/wasm32-simd/wasm32-unknown-unknown/release/makepad_example_fractal_zoom.wasm":
    location.hostname=="localhost"?
    "/makepad/target/wasm32-unknown-unknown/release/makepad_example_fractal_zoom.wasm":
    await WasmWebGL.supports_simd()? 
    "/makepad/target/wasm32-simd/wasm32-unknown-unknown/release/makepad_example_fractal_zoom.wasm":
    "/makepad/target/wasm32-thread/wasm32-unknown-unknown/release/makepad_example_fractal_zoom.wasm"
);

class MyWasmApp {
    constructor(wasm) {

        let canvas = document.getElementsByClassName('full_canvas')[0];
        this.bridge = new WasmWebGL (wasm, this, canvas);
    }
}  

let app = new MyWasmApp(wasm);
