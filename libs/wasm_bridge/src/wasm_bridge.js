export class WasmBridge {
    constructor(wasm, dispatch) {
        this.wasm = wasm;
        if(wasm === undefined){
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
        u8.set (this.view_data_u8(obj));
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
    
    chars_to_string(chars_ptr, len) {
        let out = "";
        let array = new Uint32Array(this.memory.buffer, chars_ptr, len);
        for (let i = 0; i < len; i ++) {
            out += String.fromCharCode(array[i]);
        }
        return out
    }
    
    js_console_log(chars_ptr, len) {
        console.log(this.chars_to_string(chars_ptr, len));
    }
    
    js_console_error(chars_ptr, len) {
        console.error(this.chars_to_string(chars_ptr, len), '');
    }
    
    static create_shared_memory() {
        let timeout = setTimeout(_ => {
            document.body.innerHTML = "<div style='margin-top:30px;margin-left:30px; color:white;'>Please close and re-open the browsertab - Shared memory allocation failed, this is a bug of iOS safari and apple needs to fix it.</div>"
        }, 1000)
        let mem = new WebAssembly.Memory({initial: 64, maximum: 16384, shared: true});
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
    
    static instantiate_wasm(module, memory, env) {
        let _wasm = null;
        function chars_to_string(chars_ptr, len) {
            let out = "";
            let array = new Uint32Array(wasm_for_imports._memory.buffer, chars_ptr, len);
            for (let i = 0; i < len; i ++) {
                out += String.fromCharCode(array[i]);
            }
            return out
        }
        
        env.js_console_log = (chars_ptr, len) => _wasm._bridge.js_console_log(chars_ptr, len);
        env.js_console_error = (chars_ptr, len) => _wasm._bridge.js_console_error(chars_ptr, len);
        env.js_post_signal = (hi, lo) => _wasm._bridge.js_post_signal(hi, lo);
        
        if (memory !== undefined) {
            env.memory = memory;
        }
        
        return WebAssembly.instantiate(module, {env}).then(wasm => {
            _wasm = wasm;
            wasm._has_thread_support = env.memory !== undefined;
            wasm._memory = env.memory? env.memory: wasm.exports.memory;
            wasm._module = module;
            return wasm
        }, error => {
            if (error.name == "LinkError") { // retry as multithreaded
                env.memory = this.create_shared_memory();
                return WebAssembly.instantiate(module, {env}).then(wasm => {
                    _wasm = wasm;
                    wasm._has_thread_support = true;
                    wasm._memory = env.memory;
                    wasm._module = module;
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
    
    static fetch_and_instantiate_wasm(wasm_url, memory) {
        return WebAssembly.compileStreaming(fetch(wasm_url))
            .then(
            (module) => this.instantiate_wasm(module, memory, {_post_signal: _ => {}}),
            error => {
                console.error(error)
            }
        )
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
        
        if ((u8_len & 3) != 0 || (output_ptr & 3) != 0) { // not u32 aligned, do a byte copy
            var u8_out = new Uint8Array(app.memory.buffer, output_ptr, u8_len)
            var u8_in = new Uint8Array(input_buffer)
            for (let i = 0; i < u8_len; i ++) {
                u8_out[i] = u8_in[i];
            }
        }
        else { // do a u32 copy
            let u32_len = u8_len >> 2; //4 bytes at a time.
            var u32_out = new Uint32Array(app.memory.buffer, output_ptr, u32_len)
            var u32_in = new Uint32Array(input_buffer)
            for (let i = 0; i < u32_len; i ++) {
                u32_out[i] = u32_in[i];
            }
        }
        
        app.u32[this.u32_offset ++] = output_ptr;
        app.u32[this.u32_offset ++] = u8_len;
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
        app.u32[this.u32_offset ++] = str.length;
        for (let i = 0; i < str.length; i ++) {
            app.u32[this.u32_offset ++] = str.charCodeAt(i)
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
        let len = app.u32[this.u32_offset ++];
        let str = "";
        for (let i = 0; i < len; i ++) {
            str += String.fromCharCode(app.u32[this.u32_offset ++]);
        }
        return str
    }
    
    dispatch_on_app() {
        let app = this.app;
        let u32_len = app.u32[this.u32_ptr + 1] << 1;
        while ((this.u32_offset) - this.u32_ptr < u32_len) {
            let msg_id = app.u32[this.u32_offset ++];
            this.u32_offset ++; // skip second u32 of id
            this.u32_offset ++; // skip body len
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
    for (var i = 0; i < bin.length; i ++) {
        u8[i] = bin.charCodeAt(i);
        console.log(u8[i]);
    }
    console.log(u8)
    return u8.buffer;
}
