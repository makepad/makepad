import {WasmBridge} from "/makepad/platform/wasm_bridge/src/wasm_bridge.js";

class WasmAudioWorker extends WasmBridge {
    constructor(wasm, worklet, closure_ptr) {
        super (wasm);
        this.closure_ptr = closure_ptr,
        this.worklet = worklet;
    }

    js_console_error(chars_ptr, len) {
        this.worklet.port.postMessage({
            message_type:"console_error",
            value:this.chars_to_string(chars_ptr, len)
        });   
    }

    js_console_log(chars_ptr, len) {
        this.worklet.port.postMessage({
            message_type:"console_log",
            value:this.chars_to_string(chars_ptr, len)
        });    
    }

    js_post_signal(signal_hi, signal_lo) {
        this.worklet.port.postMessage({
            message_type:"signal",
            signal_hi, signal_lo
        });
    }
    
    wasm_audio_entrypoint(frames, channels) {
        let audio_ptr = this.exports.wasm_audio_entrypoint(this.closure_ptr, frames, channels);
        this.update_array_buffer_refs();
        return audio_ptr
    }
}

class AudioWorklet extends AudioWorkletProcessor {
    constructor(options) {
        super(options);
        
        let thread_info = options.processorOptions.thread_info;

        WasmBridge.instantiate_wasm(thread_info.bytes, thread_info.memory, {}).then(wasm=>{
            wasm.instance.exports.__stack_pointer.value = thread_info.stack_ptr;
            wasm.instance.exports.__wasm_init_tls(thread_info.tls_ptr);
            let bridge = new WasmAudioWorker(wasm, this, thread_info.closure_ptr);
            this._bridge = bridge;
        }, error=>{
            console_log("Error in audio worklet" + error);
        });
    }

    process(inputs, outputs, parameters) {
        // ok great.. lets call wasm
        if(this._bridge !== undefined){
            let frames = outputs[0][0].length;
            let channels = outputs[0].length;
            let bridge = this._bridge;
            let ptr = bridge.wasm_audio_entrypoint(frames, channels);
            let ptr_f32 = ptr>>2;
            let f32 = bridge.f32;
            // lets copy the values
            for(let c = 0; c < channels; c++){
                let base = c * frames + ptr_f32;
                let out = outputs[0][c];
                for(let i = 0; i < frames; i++){
                    out[i] = f32[base + i];
                }
            }
        }
        
        return true;
    }
}

registerProcessor('audio-worklet', AudioWorklet);
