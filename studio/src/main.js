import {WasmWebGL} from "/makepad/platform/src/os/web_browser/web_gl.js";

const wasm = await WasmWebGL.fetch_and_instantiate_wasm(
    "/makepad/target/wasm32-unknown-unknown/release/makepad-studio.wasm"
);

class MyWasmApp {
    constructor(wasm) {
        let canvas = document.getElementsByClassName('full_canvas')[0];
        this.webgl = new WasmWebGL (wasm, this, canvas);
    }
} 

let app = new MyWasmApp(wasm);
