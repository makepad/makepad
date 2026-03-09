export function init_env(env) {
    let _wasm = null;

    env.js_console_log = (u8_ptr, len) => _wasm._bridge.js_console_log(u8_ptr, len);
    env.js_console_error = (u8_ptr, len) => _wasm._bridge.js_console_error(u8_ptr, len);
    env.js_time_now = () => _wasm._bridge.js_time_now();
    env.js_open_web_socket = (id, url_ptr, url_len) => console.error("js_open_web_socket out of context");
    env.js_web_socket_send_string = (id, str_ptr, url_len) => console.error("js_web_socket_send_string out of context");
    env.js_web_socket_send_binary = (id, bin_ptr, bin_len) => console.error("js_web_socket_send_binary out of context");
    env.js_network_http_request = (
        request_id_lo,
        request_id_hi,
        metadata_id_lo,
        metadata_id_hi,
        url_ptr,
        url_len,
        method_ptr,
        method_len,
        headers_ptr,
        headers_len,
        body_ptr,
        body_len
    ) => {
        if (_wasm && _wasm._bridge && _wasm._bridge.js_network_http_request) {
            _wasm._bridge.js_network_http_request(
                request_id_lo,
                request_id_hi,
                metadata_id_lo,
                metadata_id_hi,
                url_ptr,
                url_len,
                method_ptr,
                method_len,
                headers_ptr,
                headers_len,
                body_ptr,
                body_len
            );
            return;
        }
        console.error("js_network_http_request out of context");
    };
    env.js_network_http_cancel = (request_id_lo, request_id_hi) => {
        if (_wasm && _wasm._bridge && _wasm._bridge.js_network_http_cancel) {
            _wasm._bridge.js_network_http_cancel(request_id_lo, request_id_hi);
            return;
        }
        console.error("js_network_http_cancel out of context");
    };
    env.js_network_ws_open = (
        socket_id_lo,
        socket_id_hi,
        url_ptr,
        url_len,
        headers_ptr,
        headers_len
    ) => {
        if (_wasm && _wasm._bridge && _wasm._bridge.js_network_ws_open) {
            _wasm._bridge.js_network_ws_open(
                socket_id_lo,
                socket_id_hi,
                url_ptr,
                url_len,
                headers_ptr,
                headers_len
            );
            return;
        }
        console.error("js_network_ws_open out of context");
    };
    env.js_network_ws_send_binary = (
        socket_id_lo,
        socket_id_hi,
        data_ptr,
        data_len
    ) => {
        if (_wasm && _wasm._bridge && _wasm._bridge.js_network_ws_send_binary) {
            _wasm._bridge.js_network_ws_send_binary(
                socket_id_lo,
                socket_id_hi,
                data_ptr,
                data_len
            );
            return;
        }
        console.error("js_network_ws_send_binary out of context");
    };
    env.js_network_ws_send_text = (
        socket_id_lo,
        socket_id_hi,
        data_ptr,
        data_len
    ) => {
        if (_wasm && _wasm._bridge && _wasm._bridge.js_network_ws_send_text) {
            _wasm._bridge.js_network_ws_send_text(
                socket_id_lo,
                socket_id_hi,
                data_ptr,
                data_len
            );
            return;
        }
        console.error("js_network_ws_send_text out of context");
    };
    env.js_network_ws_close = (socket_id_lo, socket_id_hi) => {
        if (_wasm && _wasm._bridge && _wasm._bridge.js_network_ws_close) {
            _wasm._bridge.js_network_ws_close(socket_id_lo, socket_id_hi);
            return;
        }
        console.error("js_network_ws_close out of context");
    };

    return (wasm) => { _wasm = wasm };
}

export class WasmBridge {
    static SPLIT_DATA_VERSION = 2;
    static SPLIT_SLOT_EXPORT_PREFIX = "$s";

    constructor(wasm, dispatch) {
        this.wasm = wasm;
        if (wasm === undefined) {
            return console.error("Wasm object is undefined, check your URL and build output")
        }
        this.wasm._bridge = this;
        this.dispatch = dispatch;
        this.exports = wasm.exports;
        this.memory = wasm._memory;
        this.wasm_url = wasm._wasm_url;
        this.buffer_ref_len_check = 0;

        this.from_wasm_args = {};

        this.update_array_buffer_refs();

        this.wasm_init_panic_hook();
    }

    create_js_message_bridge(wasm_app) {
        let msg = new FromWasmMsg(this, this.wasm_get_js_message_bridge(wasm_app));
        let code = msg.read_str();
        msg.free();
        // this class can also be loaded from file.
        this.msg_class = new Function("ToWasmMsg", "FromWasmMsg", code)(ToWasmMsg, FromWasmMsg);
    }

    clear_memory_refs() {
        this.exports = null;
        this.memory = null;
        this.wasm._memory = null;
        this.f32 = null;
        this.u32 = null;
        this.f64 = null;
        this.wasm = null;
    }

    update_array_buffer_refs() {
        if (this.buffer_ref_len_check != this.memory.buffer.byteLength) {
            this.f32 = new Float32Array(this.memory.buffer);
            this.u32 = new Uint32Array(this.memory.buffer);
            this.f64 = new Float64Array(this.memory.buffer);
            this.buffer_ref_len_check = this.memory.buffer.byteLength;
        }
    }

    new_to_wasm() {
        return new this.msg_class.ToWasmMsg(this);
    }

    new_from_wasm(ptr) {
        return new this.msg_class.FromWasmMsg(this, ptr);
    }

    clone_data_u8(obj) {
        var dst = new ArrayBuffer(obj.len);
        let u8 = new Uint8Array(dst);
        u8.set(this.view_data_u8(obj));
        return u8;
    }

    view_data_u8(obj) {
        return new Uint8Array(this.memory.buffer, obj.ptr, obj.len)
    }

    free_data_u8(obj) {
        this.wasm_free_data_u8(obj.ptr, obj.len, obj.capacity);
    }

    wasm_get_js_message_bridge(wasm_app) {
        let new_ptr = this.exports.wasm_get_js_message_bridge(wasm_app);
        this.update_array_buffer_refs();
        return new_ptr
    }

    wasm_new_msg_with_u64_capacity(capacity) {
        let new_ptr = this.exports.wasm_new_msg_with_u64_capacity(capacity)
        this.update_array_buffer_refs();
        return new_ptr
    }

    wasm_msg_reserve_u64(ptr, capacity) {
        let new_ptr = this.exports.wasm_msg_reserve_u64(ptr, capacity);
        this.update_array_buffer_refs();
        return new_ptr
    }

    wasm_msg_free(ptr) {
        this.exports.wasm_msg_free(ptr);
        this.update_array_buffer_refs();
    }

    wasm_new_data_u8(capacity) {
        let new_ptr = this.exports.wasm_new_data_u8(capacity);
        this.update_array_buffer_refs();
        return new_ptr
    }

    wasm_free_data_u8(ptr, len, cap) {
        this.exports.wasm_free_data_u8(ptr, len, cap);
        this.update_array_buffer_refs();
    }

    wasm_init_panic_hook() {
        this.exports.wasm_init_panic_hook();
        this.update_array_buffer_refs();
    }
    /*
    chars_to_string(chars_ptr, len) {
        let out = "";
        let array = new Uint32Array(this.memory.buffer, chars_ptr, len);
        for (let i = 0; i < len; i ++) {
            out += String.fromCharCode(array[i]);
        }
        return out
    }*/

    u8_to_string(ptr, len) {
        let u8 = new Uint8Array(this.memory.buffer, ptr, len);
        let copy = new Uint8Array(len);
        copy.set(u8);
        const decoder = new TextDecoder();
        return decoder.decode(copy);
    }

    js_console_log(u8_ptr, len) {
        console.log(this.u8_to_string(u8_ptr, len));
    }

    js_console_error(u8_ptr, len) {
        console.error(this.u8_to_string(u8_ptr, len), '');
    }

    js_time_now() {
        return Date.now() / 1000.0;
    }

    static create_shared_memory() {
        let timeout = setTimeout(_ => {
            document.body.innerHTML = "<div style='margin-top:30px;margin-left:30px; color:white;'>Please close and re-open the browsertab - Shared memory allocation failed, this is a bug of iOS safari and apple needs to fix it.</div>"
        }, 1000)
        let mem = new WebAssembly.Memory({ initial: 64, maximum: 16384, shared: true });
        clearTimeout(timeout);
        return mem;
    }

    static async supports_simd() {
        let bytes = Uint8Array.from([0, 97, 115, 109, 1, 0, 0, 0, 1, 5, 1, 96, 0, 1, 123, 3, 2, 1, 0, 10, 10, 1, 8, 0, 65, 0, 253, 15, 253, 98, 11, 0, 10, 4, 110, 97, 109, 101, 2, 3, 1, 0, 0]);
        return WebAssembly.instantiate(bytes).then(_ => {
            return true
        }, _ => {
            return false
        })
    }

    static parse_split_data_blob(bytes) {
        const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
        if (bytes.byteLength < 12) {
            throw new Error("split data blob too small");
        }
        if (String.fromCharCode(bytes[0], bytes[1], bytes[2], bytes[3]) !== "MPDS") {
            throw new Error("invalid split data blob header");
        }
        const version = view.getUint32(4, true);
        if (version !== this.SPLIT_DATA_VERSION) {
            throw new Error("unsupported split data blob version");
        }
        const count = view.getUint32(8, true);
        let offset = 12;
        const segments = [];
        for (let i = 0; i < count; i++) {
            if (offset + 13 > bytes.byteLength) {
                throw new Error("truncated split data segment header");
            }
            const kind = bytes[offset];
            offset += 1;
            const memory_index = view.getUint32(offset, true);
            offset += 4;
            const address = view.getUint32(offset, true);
            offset += 4;
            const len = view.getUint32(offset, true);
            offset += 4;
            if (offset + len > bytes.byteLength) {
                throw new Error("truncated split data segment payload");
            }
            segments.push({
                kind,
                memory_index,
                offset: address,
                bytes: bytes.slice(offset, offset + len),
            });
            offset += len;
        }
        return { version, segments };
    }

    static read_var_u32(bytes, offset) {
        let result = 0;
        let shift = 0;
        while (offset < bytes.length) {
            const byte = bytes[offset++];
            result |= (byte & 0x7f) << shift;
            if ((byte & 0x80) === 0) {
                return { value: result >>> 0, offset };
            }
            shift += 7;
        }
        throw new Error("truncated var_u32");
    }

    static encode_var_u32(value) {
        const out = [];
        do {
            let byte = value & 0x7f;
            value >>>= 7;
            if (value !== 0) {
                byte |= 0x80;
            }
            out.push(byte);
        } while (value !== 0);
        return out;
    }

    static encode_var_i32(value) {
        const out = [];
        while (true) {
            let byte = value & 0x7f;
            value >>= 7;
            const done = (value === 0 && (byte & 0x40) === 0) || (value === -1 && (byte & 0x40) !== 0);
            if (done) {
                out.push(byte);
                return out;
            }
            out.push(byte | 0x80);
        }
    }

    static encode_split_data_section_payload(segments) {
        const payload = [];
        payload.push(...this.encode_var_u32(segments.length));
        for (const segment of segments) {
            if (segment.kind === 1) {
                payload.push(1);
                payload.push(...this.encode_var_u32(segment.bytes.length));
                for (const byte of segment.bytes) {
                    payload.push(byte);
                }
            } else {
                if (segment.memory_index === 0) {
                    payload.push(0);
                } else {
                    payload.push(2);
                    payload.push(...this.encode_var_u32(segment.memory_index));
                }
                payload.push(0x41);
                payload.push(...this.encode_var_i32(segment.offset | 0));
                payload.push(0x0b);
                payload.push(...this.encode_var_u32(segment.bytes.length));
                for (const byte of segment.bytes) {
                    payload.push(byte);
                }
            }
        }
        return new Uint8Array(payload);
    }

    static rebuild_split_wasm(wasm_bytes, segments) {
        const data_payload = this.encode_split_data_section_payload(segments);
        let offset = 8;
        while (offset < wasm_bytes.length) {
            const section_start = offset;
            const type_id = wasm_bytes[offset++];
            const payload_len_info = this.read_var_u32(wasm_bytes, offset);
            const payload_start = payload_len_info.offset;
            const payload_end = payload_start + payload_len_info.value;
            if (payload_end > wasm_bytes.length) {
                throw new Error("truncated wasm section payload");
            }
            if (type_id === 11) {
                const encoded_len = Uint8Array.from(this.encode_var_u32(data_payload.length));
                const rebuilt = new Uint8Array(
                    section_start + 1 + encoded_len.length + data_payload.length + (wasm_bytes.length - payload_end)
                );
                rebuilt.set(wasm_bytes.slice(0, section_start), 0);
                let out_offset = section_start;
                rebuilt[out_offset++] = 11;
                rebuilt.set(encoded_len, out_offset);
                out_offset += encoded_len.length;
                rebuilt.set(data_payload, out_offset);
                out_offset += data_payload.length;
                rebuilt.set(wasm_bytes.slice(payload_end), out_offset);
                return rebuilt;
            }
            offset = payload_end;
        }
        throw new Error("split wasm missing data section");
    }

    static patch_split_table(primary_exports, secondary_exports) {
        const split_table = primary_exports.$s;
        if (!(split_table instanceof WebAssembly.Table)) {
            throw new Error("primary wasm missing $s export");
        }

        for (const [name, value] of Object.entries(secondary_exports)) {
            if (!name.startsWith(this.SPLIT_SLOT_EXPORT_PREFIX)) {
                continue;
            }
            const slot = Number.parseInt(name.slice(this.SPLIT_SLOT_EXPORT_PREFIX.length), 10);
            if (!Number.isInteger(slot)) {
                continue;
            }
            split_table.set(slot, value);
        }
    }

    static async compile_primary_module(wasm_bytes, split_response_promise) {
        if (!split_response_promise) {
            return WebAssembly.compile(wasm_bytes);
        }
        const split_response = await split_response_promise;
        if (!split_response.ok) {
            throw new Error(`failed to fetch split data: ${split_response.status}`);
        }
        const split_bytes = new Uint8Array(await split_response.arrayBuffer());
        const split = this.parse_split_data_blob(split_bytes);
        const rebuilt_bytes = this.rebuild_split_wasm(wasm_bytes, split.segments);
        return WebAssembly.compile(rebuilt_bytes);
    }

    static async attach_secondary_wasm(primary_wasm, secondary_response_promise, defer_secondary) {
        if (!secondary_response_promise) {
            return;
        }
        primary_wasm._secondary_ready = (async () => {
            const secondary_response = await secondary_response_promise;
            if (!secondary_response.ok) {
                throw new Error(`failed to fetch secondary wasm: ${secondary_response.status}`);
            }
            await this._instantiate_secondary(secondary_response, primary_wasm);
        })();
        if (!defer_secondary) {
            await primary_wasm._secondary_ready;
        }
    }

    static instantiate_wasm(module, memory, env) {
        let set_wasm = init_env(env);

        if (memory !== undefined) {
            env.memory = memory;
        }

        return WebAssembly.instantiate(module, { env }).then(async wasm => {
            set_wasm(wasm);
            wasm._has_thread_support = env.memory !== undefined;
            wasm._memory = env.memory ? env.memory : wasm.exports.memory;
            wasm._module = module;
            wasm._env = env;
            return wasm
        }, error => {
            if (error.name == "LinkError") { // retry as multithreaded
                env.memory = this.create_shared_memory();
                return WebAssembly.instantiate(module, { env }).then(async wasm => {
                    set_wasm(wasm);
                    wasm._has_thread_support = true;
                    wasm._memory = env.memory;
                    wasm._module = module;
                    wasm._env = env;
                    return wasm
                }, error => {
                    console.error(error);
                    return error
                })
            }
            else {
                console.error(error);
                return error
            }
        })
    }

    static fetch_and_instantiate_wasm(wasm_url, memory, split_config) {
        const has_split_data = split_config && split_config.split_data_url;
        const has_secondary = split_config && split_config.secondary_wasm_url;
        const defer_secondary = !!(split_config && split_config.defer_secondary_wasm);

        if (has_split_data || has_secondary) {
            return (async () => {
                const wasm_response_promise = fetch(wasm_url);
                const split_response_promise = has_split_data
                    ? fetch(split_config.split_data_url)
                    : null;
                const secondary_response_promise = has_secondary
                    ? fetch(split_config.secondary_wasm_url)
                    : null;

                const wasm_response = await wasm_response_promise;
                if (!wasm_response.ok) {
                    throw new Error(`failed to fetch wasm: ${wasm_response.status}`);
                }

                const wasm_bytes = new Uint8Array(await wasm_response.arrayBuffer());
                const module = await this.compile_primary_module(wasm_bytes, split_response_promise);
                const wasm = await this.instantiate_wasm(module, memory, { _post_signal: _ => { } });
                await this.attach_secondary_wasm(wasm, secondary_response_promise, defer_secondary);
                return wasm;
            })().catch(error => {
                console.error(error);
            });
        }
        return WebAssembly.compileStreaming(fetch(wasm_url))
            .then(
                (module) => this.instantiate_wasm(module, memory, { _post_signal: _ => { } }),
                error => {
                    console.error(error)
                }
            )
    }

    static async _instantiate_secondary(secondary_response, primary_wasm) {
        const secondary_bytes = await secondary_response.arrayBuffer();
        const secondary_module = await WebAssembly.compile(secondary_bytes);
        primary_wasm._secondary_module = secondary_module;
        const imports = {
            env: primary_wasm._env || {},
            $p: primary_wasm.exports
        };
        const secondary_instance = await WebAssembly.instantiate(secondary_module, imports);
        this.patch_split_table(primary_wasm.exports, secondary_instance.exports);
    }
}

export class ToWasmMsg {
    constructor(app) {
        this.app = app
        this.ptr = app.wasm_new_msg_with_u64_capacity(1024);
        this.u32_ptr = this.ptr >> 2;
        this.u32_offset = this.u32_ptr + 2;
        this.u32_needed_capacity = 0; //app.u32[this.u32_ptr] << 1;
    }

    reserve_u32(u32_capacity) {
        let app = this.app;

        this.u32_needed_capacity += u32_capacity;
        let u64_needed_capacity = ((this.u32_needed_capacity & 1) + this.u32_needed_capacity) >> 1;
        let offset = this.u32_offset - this.u32_ptr;
        let u64_len = ((offset & 1) + offset) >> 1;

        if (app.u32[this.u32_ptr] - u64_len < u64_needed_capacity) {
            app.u32[this.u32_ptr + 1] = u64_len;
            this.ptr = this.app.wasm_msg_reserve_u64(this.ptr, u64_needed_capacity);
            this.u32_ptr = this.ptr >> 2;
            this.u32_offset = this.u32_ptr + offset;
        }
    }

    // i forgot how to do memcpy with typed arrays. so, we'll do this.
    push_data_u8(input_buffer) {


        let app = this.app;

        let u8_len = input_buffer.byteLength;
        let output_ptr = app.wasm_new_data_u8(u8_len);
        var u8_out = new Uint8Array(app.memory.buffer, output_ptr, u8_len)
        var u8_in = new Uint8Array(input_buffer)

        u8_out.set(u8_in);

        app.u32[this.u32_offset++] = output_ptr;
        app.u32[this.u32_offset++] = u8_len;
    }

    release_ownership() {
        if (this.ptr === 0) {
            throw new Error("double finalise")
        }
        let app = this.app;
        let ptr = this.ptr;
        let offset = this.u32_offset - this.u32_ptr;

        if ((offset & 1) != 0) {
            app.u32[this.u32_offset + 1] = 0
        }

        let u64_len = ((offset & 1) + offset) >> 1;
        app.u32[this.u32_ptr + 1] = u64_len;

        this.app = null;
        this.ptr = 0;
        this.u32_ptr = 0;
        this.u32_offset = 0;
        this.u32_needed_capacity = 0;

        return ptr;
    }

    push_str(str) {
        let app = this.app;
        this.reserve_u32(str.length + 1);
        app.u32[this.u32_offset++] = str.length;
        for (let i = 0; i < str.length; i++) {
            app.u32[this.u32_offset++] = str.charCodeAt(i)
        }
    }
}

export class FromWasmMsg {
    constructor(app, ptr) {
        this.app = app
        this.ptr = ptr;
        this.u32_ptr = this.ptr >> 2;
        this.u32_offset = this.u32_ptr + 2;
    }

    free() {
        let app = this.app;
        app.wasm_msg_free(this.ptr);
        this.app = null;
        this.ptr = 0;
        this.u32_ptr = 0;
        this.u32_offset = 0;
    }

    read_str() {
        let app = this.app;
        let len = app.u32[this.u32_offset++];
        let str = "";
        for (let i = 0; i < len; i++) {
            str += String.fromCharCode(app.u32[this.u32_offset++]);
        }
        return str
    }

    dispatch_on_app() {
        let app = this.app;
        let u32_len = app.u32[this.u32_ptr + 1] << 1;
        while ((this.u32_offset) - this.u32_ptr < u32_len) {
            let msg_id = app.u32[this.u32_offset++];
            this.u32_offset++; // skip second u32 of id
            this.u32_offset++; // skip body len
            // dispatch to deserializer
            if (this[msg_id] !== undefined) {
                this[msg_id]();
            }
            else {
                this.dispatch[msg_id]()
            }
            this.u32_offset += this.u32_offset & 1; // align
        }
    }
}

function base64_to_array_buffer(base64) {
    var bin = window.atob(base64);
    var u8 = new Uint8Array(bin.length);
    for (var i = 0; i < bin.length; i++) {
        u8[i] = bin.charCodeAt(i);
        console.log(u8[i]);
    }
    console.log(u8)
    return u8.buffer;
}
