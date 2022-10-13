import {WasmMediaGL} from "/makepad/media/src/os/web_browser/media_gl.js"

const wasm = await WasmMediaGL.fetch_and_instantiate_wasm(
    location.hostname=="127.0.0.1"?
    "/makepad/target/wasm32-thread/wasm32-unknown-unknown/release/makepad-example-ironfish.wasm":
    location.hostname=="localhost"?
    "/makepad/target/wasm32-unknown-unknown/release/makepad-example-ironfish.wasm":
    "/makepad/target/wasm32-thread/wasm32-unknown-unknown/release/makepad-example-ironfish.wasm"
);

class MyWasmApp {
    constructor(wasm) {
        let canvas = document.getElementsByClassName('full_canvas')[0];
        this.webgl = new WasmMediaGL (wasm, this, canvas);
    }
} 

let app = new MyWasmApp(wasm);

