const AUDIO_WORKLET_SIGNAL_BATCHING = 8;
const SPLIT_SLOT_EXPORT_PREFIX = "$s";

function patch_split_table(primary_exports, secondary_exports) {
    const split_table = primary_exports.$s;
    if (!(split_table instanceof WebAssembly.Table)) {
        throw new Error("primary wasm missing $s export");
    }
    for (const [name, value] of Object.entries(secondary_exports)) {
        if (!name.startsWith(SPLIT_SLOT_EXPORT_PREFIX)) {
            continue;
        }
        const slot = Number.parseInt(name.slice(SPLIT_SLOT_EXPORT_PREFIX.length), 10);
        if (!Number.isInteger(slot)) {
            continue;
        }
        split_table.set(slot, value);
    }
}

class AudioWorklet extends AudioWorkletProcessor {
    constructor(options) {
        super(options);

        let thread_info = options.processorOptions.thread_info;

        const instantiate_secondary = async (primary_wasm, env) => {
            if (!thread_info.secondary_module) {
                return;
            }
            const secondary_instance = await WebAssembly.instantiate(thread_info.secondary_module, {
                env,
                $p: primary_wasm.exports
            });
            patch_split_table(primary_wasm.exports, secondary_instance.exports);
        };

        function chars_to_string(chars_ptr, len) {
            let out = "";
            let array = new Uint32Array(thread_info.memory.buffer, chars_ptr, len);
            for (let i = 0; i < len; i++) {
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
            js_web_socket_send_string(id, str_ptr, str_len) {
            },

            js_web_socket_send_binary(id, bin_ptr, bin_len) {
            },

            js_open_web_socket: (id, url_ptr, url_len) => {

            }
        };
        WebAssembly.instantiate(thread_info.module, { env }).then(async wasm => {
            await instantiate_secondary(wasm, env);

            wasm.exports.__stack_pointer.value = thread_info.stack_ptr;
            wasm.exports.__wasm_init_tls(thread_info.tls_ptr);

            this._context = {
                exports: wasm.exports,
                memory: env.memory,
                context_ptr: thread_info.context_ptr,
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

            let frames = outputs[0][0].length;
            let channels = outputs[0].length;

            let output_ptr = context.exports.wasm_audio_output_entrypoint(context.context_ptr, frames, channels);

            if (context.buffer_ref_len_check != context.memory.buffer.byteLength) {
                context.f32 = new Float32Array(context.memory.buffer);
                context.buffer_ref_len_check = context.memory.buffer.byteLength;
            }

            let ptr_f32 = output_ptr >> 2;
            let f32 = context.f32;
            // lets copy the values from wasm to the output buffer
            for (let c = 0; c < channels; c++) {
                let base = c * frames + ptr_f32;
                let out = outputs[0][c];
                for (let i = 0; i < frames; i++) {
                    out[i] = f32[base + i];
                }
            }
        }
        return true;
    }
}

registerProcessor('audio-worklet', AudioWorklet);
