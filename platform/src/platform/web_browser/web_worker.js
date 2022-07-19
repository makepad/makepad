import {WasmBridge} from "/makepad/platform/wasm_bridge/src/wasm_bridge.js";

class WasmWorker extends WasmBridge {
    constructor(wasm) {
        super (wasm);
    }
    
    js_post_signal(signal_hi, signal_lo) {
        postMessage({signal_hi, signal_lo});
    }
}

onmessage = async function(e) {
    let thread_info = e.data;
    let wasm = await WasmBridge.instantiate_wasm(thread_info.bytes, thread_info.memory, {});
    
    wasm.instance.exports.__stack_pointer.value = thread_info.stack_ptr;
    wasm.instance.exports.__wasm_init_tls(thread_info.tls_ptr);
    
    let bridge = new WasmWorker(wasm);

    wasm.instance.exports.wasm_thread_entrypoint(thread_info.closure_ptr);
    
    // so depending on what you return here we can sleep the webworker and re-enter
    console.log("terminating worker");
    close();
}
