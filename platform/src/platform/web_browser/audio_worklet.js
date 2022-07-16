class AudioWorklet extends AudioWorkletProcessor {
    constructor(options) {
        super (options);
        
        let thread_info = options.processorOptions.thread_info;
        
        function chars_to_string(chars_ptr, len) {
            let out = "";
            let array = new Uint32Array(thread_info.memory.buffer, chars_ptr, len);
            for (let i = 0; i < len; i ++) {
                out += String.fromCharCode(array[i]);
            }
            return out
        }
        
        let env = {
            memory: thread_info.memory,
            
            js_console_error: (chars_ptr, len) => {
                this.port.postMessage({
                    message_type: "console_error",
                    value: chars_to_string(chars_ptr, len)
                });
            },
            
            js_console_log: (chars_ptr, len) => {
                this.port.postMessage({
                    message_type: "console_log",
                    value: chars_to_string(chars_ptr, len)
                });
            },
            
            js_post_signal: (signal_hi, signal_lo) => {
                this.port.postMessage({
                    message_type: "signal",
                    signal_hi,
                    signal_lo
                });
            }
        };
        
        WebAssembly.instantiate(thread_info.bytes, {env}).then(wasm => {
            
            wasm.instance.exports.__stack_pointer.value = thread_info.stack_ptr;
            wasm.instance.exports.__wasm_init_tls(thread_info.tls_ptr);

            this._closure_ptr = thread_info.closure_ptr;
            this._wasm = wasm;
            this._memory = env.memory;
        }, error => {
            this.port.postMessage({
                message_type: "console_error",
                value: "Cannot instantiate wasm" + error
            });
        })
    }
    _update_array_buffer_refs() {
        if (this._buffer_ref_len_check != this._memory.buffer.byteLength) {
            this._f32 = new Float32Array(this._memory.buffer);
            this._buffer_ref_len_check = this._memory.buffer.byteLength;
        }
    }
    
    process(inputs, outputs, parameters) {
        if (this._wasm !== undefined) {
            let frames = outputs[0][0].length;
            let channels = outputs[0].length;
            
            let ptr = this._wasm.instance.exports.wasm_audio_entrypoint(this._closure_ptr, frames, channels);
            this._update_array_buffer_refs();
            
            let ptr_f32 = ptr >> 2;
            let f32 = this._f32;
            // lets copy the values
            for (let c = 0; c < channels; c ++) {
                let base = c * frames + ptr_f32;
                let out = outputs[0][c];
                for (let i = 0; i < frames; i ++) {
                    out[i] = f32[base + i];
                }
            }
        }
        return true;
    }
}

registerProcessor('audio-worklet', AudioWorklet);
