import {WasmApp} from "/makepad/platform/wasm_bridge/src/wasm_app.js"

export class WebGLWasmApp extends WasmApp{
    constructor(canvas, wasm) {
        super(wasm);
        
        this.canvas = canvas;
        
    }
}
