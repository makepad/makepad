import {WebGLWasmApp} from "/makepad/platform/src/platform/webbrowser/webgl_app.js"

let canvas = document.getElementsByClassName('main_canvas')[0];

class MyWasmApp {
    constructor(wasm, canvas){
        this.app = new WebGLWasmApp (wasm, this, canvas);
    }
}

const wasm = await WebGLWasmApp.load_wasm_from_url("/makepad/target/wasm32-unknown-unknown/debug/layout_example.wasm"); 

let app = new MyWasmApp(wasm, canvas);
