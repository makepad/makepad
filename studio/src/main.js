import {WasmWebGL} from "/makepad/platform/src/platform/web_browser/web_gl.js";

const wasm = await WasmWebGL.fetch_and_instantiate_wasm(
    "/makepad/target/wasm32-unknown-unknown/debug/makepad_studio.wasm"
);

class MyWasmApp {
    constructor(wasm) {
        let canvas = document.getElementsByClassName('full_canvas')[0];
        this.webgl = new WasmWebGL (wasm, this, canvas);
    }
} 

let app = new MyWasmApp(wasm);
