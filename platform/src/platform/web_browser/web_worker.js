import {WasmBridge} from "/makepad/platform/wasm_bridge/src/wasm_bridge.js"

class WasmWorker extends WasmBridge{
    constructor(wasm){
        super(wasm);
    }
}

onmessage = async function(e) { 
    let data = e.data;
    let wasm = await WasmBridge.instantiate_wasm(data.bytes, data.memory, function post_signal(signal_id, data_hi, data_lo){
        postMessage({signal_id, data_hi, data_lo});
    });
    let bridge = new WasmWorker(wasm);

    console.log(bridge.exports);
    bridge.exports.__stack_pointer.value = data.stack_ptr;
    bridge.exports.__wasm_init_tls(data.tls_ptr);
    
    bridge.exports.wasm_thread_entrypoint(data.closure_ptr);
}
