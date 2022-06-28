import {WasmApp} from "/makepad/platform/wasm_msg/src/wasm_msg.js"

export class WebGLWasmApp extends WasmApp{
    constructor(canvas, wasm) {
        super(wasm);
        
        this.canvas = canvas;
        
    }
}
