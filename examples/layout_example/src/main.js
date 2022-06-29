import {WebGLWasmApp} from "/makepad/platform/src/platform/webbrowser/webgl_platform.js"

let canvas = document.getElementsByClassName('main_canvas')[0];

class MyWasmApp extends WebGLWasmApp {
}

MyWasmApp.load_wasm_from_url(
    "/makepad/target/wasm32-unknown-unknown/debug/layout_example.wasm",
    (wasm) => {
        let app = new MyWasmApp(canvas, wasm);
    },
    (err) => {
    }
);

