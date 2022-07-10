import {WasmWebGL} from "/makepad/platform/src/platform/web_browser/web_gl.js"
const wasm = await WasmWebGL.load_wasm("/makepad/target/wasm32-unknown-unknown/debug/fun_audio.wasm"); 

class MyWasmApp {
    constructor(wasm){
        let canvas = document.getElementsByClassName('main_canvas')[0];
        this.webgl = new WasmWebGL (wasm, this, canvas);
    }
}

let app = new MyWasmApp(wasm);
