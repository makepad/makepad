import {WasmWebGL} from "/makepad/platform/src/platform/web_browser/web_gl.js"

const wasm = await WasmWebGL.fetch_and_instantiate_wasm(
    location.hostname=="127.0.0.1"?
    "/makepad/target/wasm32-thread/wasm32-unknown-unknown/release/fun_audio.wasm":
    location.hostname=="localhost"?
    "/makepad/target/wasm32-unknown-unknown/release/fun_audio.wasm":
    "/makepad/target/wasm32-unknown-unknown/release/fun_audio.wasm"
);

class MyWasmApp {
    constructor(wasm) {
        let canvas = document.getElementsByClassName('full_canvas')[0];
        this.webgl = new WasmWebGL (wasm, this, canvas);
    }
} 

let app = new MyWasmApp(wasm);
