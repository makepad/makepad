import {WasmWebGL} from "/makepad/platform/src/platform/web_browser/webgl.js"

let canvas = document.getElementsByClassName('main_canvas')[0];

class MyWasmApp {
    constructor(wasm, canvas){
        this.app = new WasmWebGL (wasm, this, canvas);
    }
}

const wasm = await WasmWebGL.load_wasm_from_url("/makepad/target/wasm32-unknown-unknown/debug/layout_example.wasm"); 

let app = new MyWasmApp(wasm, canvas);
