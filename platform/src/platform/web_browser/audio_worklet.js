import {WasmBridge} from "/makepad/platform/wasm_bridge/src/wasm_bridge.js";

let global_worklet = null;
function console_log(str){
    global_worklet.port.postMessage({
        message_type:"console_log",
        value:str
    });    
}

function console_error(str){
    global_worklet.port.postMessage({
        message_type:"console_error",
        value:str
    });    
}

class WasmAudioWorker extends WasmBridge {
    constructor(wasm) {
        super (wasm);
    }

    js_console_error(chars_ptr, len) {
        console_error(this.chars_to_string(chars_ptr, len))
    }

    js_console_log(chars_ptr, len) {
        console_log(this.chars_to_string(chars_ptr, len))
    }

    js_post_signal(signal_hi, signal_lo) {
        this._worklet.port.postMessage({
            message_type:"signal",
            signal_hi, signal_lo
        });
    }
}



class AudioWorklet extends AudioWorkletProcessor {
    constructor(options) {
        super(options);
        global_worklet = this;
        
        let thread_info = options.processorOptions.thread_info;
        
        WasmBridge.instantiate_wasm(thread_info.bytes, thread_info.memory, {}).then(wasm=>{
            wasm.instance.exports.__stack_pointer.value = thread_info.stack_ptr;
            wasm.instance.exports.__wasm_init_tls(thread_info.tls_ptr);
            let bridge = new WasmAudioWorker(wasm);
            this._wasm = wasm;
            this._closure_ptr = thread_info.closure_ptr;
            bridge._worklet = this;
        }, error=>{
            console_log("Error in audio worklet" + error);
        });
        //wasm.instance.exports.wasm_thread_entrypoint(data.closure_ptr);
    }

    //static get parameterDescriptors() {
        //return [];
    //}
    
    process(inputs, outputs, parameters) {
        //console_log("BLOCK");
        // ok great.. lets call wasm
        if(this._wasm !== undefined){
            this._wasm.instance.exports.wasm_audio_entrypoint(this._closure_ptr);
        }
        /*const output = outputs[0];

        const amplitude = parameters.amplitude;
        const isAmplitudeConstant = amplitude.length === 1;
        
        for (let channel = 0; channel < output.length; ++ channel) {
            const outputChannel = output[channel];
            for (let i = 0; i < outputChannel.length; ++ i) {
                // This loop can branch out based on AudioParam array length, but
                // here we took a simple approach for the demonstration purpose.
                outputChannel[i] = 2 * (Math.random() - 0.5) *
                (isAmplitudeConstant? amplitude[0]: amplitude[i]);
            }
        }
        */
        return true;
    }
}

registerProcessor('audio-worklet', AudioWorklet);
