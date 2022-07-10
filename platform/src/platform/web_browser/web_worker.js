import {WasmBridge} from "/makepad/platform/wasm_bridge/src/wasm_bridge.js"

console.log("Hello from worker")

class WasmWorker extends WasmBridge{
    constructor(wasm){
        super(wasm);
    }
}

onmessage = async function(e) { 
    let data = e.data;
    let wasm = await WasmBridge.instantiate_wasm(data.bytes, data.memory);
    let bridge = new WasmWorker(wasm);
    bridge.exports.wasm_thread_entrypoint();
}
