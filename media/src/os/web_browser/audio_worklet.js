const AUDIO_WORKLET_SIGNAL_BATCHING = 8;

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
                // unswamp our messageloop 
                let found = false;
                for (let i = 0; i < this._context.signals_lo.length; i++){
                    let sh = this._context.signals_hi[i];
                    let sl = this._context.signals_lo[i];
                    if(sh == signal_hi && sl == signal_lo){
                        found = true
                    }
                }
                if(!found){
                    this._context.signals_hi.push(signal_hi);
                    this._context.signals_lo.push(signal_lo);
                }
            }
        };
        
        WebAssembly.instantiate(thread_info.module, {env}).then(wasm => {
            
            wasm.exports.__stack_pointer.value = thread_info.stack_ptr;
            wasm.exports.__wasm_init_tls(thread_info.tls_ptr);
            
            this._context = {
                signals_lo: [],
                signals_hi: [],
                signal_counter: 0,
                exports: wasm.exports,
                memory: env.memory,
                closure_ptr: thread_info.closure_ptr,
                flush_signals_in_audio_flow: ()=>{
                    this._context.signal_counter += 1;
                    if (this._context.signal_counter >= AUDIO_WORKLET_SIGNAL_BATCHING) {
                        this.port.postMessage({
                            message_type: "signal",
                            signals_hi: this._context.signals_hi,
                            signals_lo: this._context.signals_lo,
                        });
                        this._context.signals_lo.length = 0;
                        this._context.signals_hi.length = 0;
                        this._context.signal_counter = 0;
                    }
                }
            }
        }, error => {
            this.port.postMessage({
                message_type: "console_error",
                value: "Cannot instantiate wasm" + error
            });
        })
    }
    
    process(inputs, outputs, parameters) {
        if (this._context !== undefined) {
            let context = this._context;
            
            context.flush_signals_in_audio_flow();
            
            let frames = outputs[0][0].length;
            let channels = outputs[0].length;
            
            let ptr = context.exports.wasm_audio_entrypoint(context.closure_ptr, frames, channels);
            if (context.buffer_ref_len_check != context.memory.buffer.byteLength) {
                context.f32 = new Float32Array(context.memory.buffer);
                context.buffer_ref_len_check = context.memory.buffer.byteLength;
            }
            
            let ptr_f32 = ptr >> 2;
            let f32 = context.f32;
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
