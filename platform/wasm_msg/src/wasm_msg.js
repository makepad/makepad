export class WasmApp {
    constructor(wasm) {
        this.wasm = wasm;
        this.exports = wasm.instance.exports;
        this.memory = wasm.instance.exports.memory;
        
        this.buffer_ref_len_check = 0;
        
        this.from_wasm_args = {};
        
        this.update_array_buffer_refs();
        
        let msg = new FromWasmMsg(this, this.get_wasm_js_msg_impl());
        let code = msg.read_str();
        msg.destroy();
        
        // this class can also be loaded from file.
        this.msg_class = new Function("ToWasmMsg", "FromWasmMsg", code)(ToWasmMsg, FromWasmMsg);
        console.log(this.msg_class)
    }
    
    update_array_buffer_refs() {
        if (this.buffer_ref_len_check != this.exports.memory.buffer.byteLength) {
            this.f32 = new Float32Array(this.exports.memory.buffer);
            this.u32 = new Uint32Array(this.exports.memory.buffer);
            this.f64 = new Float64Array(this.exports.memory.buffer);
            this.buffer_ref_len_check = this.exports.memory.buffer.byteLength;
        }
    }
    
    get_wasm_js_msg_impl() {
        let new_ptr = this.exports.get_wasm_js_msg_impl();
        this.update_array_buffer_refs();
        return new_ptr
    }
    
    new_wasm_msg_with_u64_capacity(capacity) {
        let new_ptr = this.exports.new_wasm_msg_with_u64_capacity(capacity)
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
    
    new_to_wasm_data_u8(capacity){
        let new_ptr = this.exports.new_to_wasm_data_u8(capacity);
        this.update_array_buffer_refs();
        return new_ptr        
    }
    
    process_to_wasm_msg(msg_ptr) {
        let ret_ptr = this.exports.process_to_wasm_msg(msg_ptr)
        this.update_array_buffer_refs();
        return ret_ptr
    }
    
    to_wasm_pump(to_wasm) {
        let ret_ptr = this.process_to_wasm_msg(to_wasm.finalise());
        let from_wasm = new this.msg_class.FromWasmMsg(this, ret_ptr);
        from_wasm.dispatch();
        from_wasm.destroy();
    }
    
    new_to_wasm() {
        return new this.msg_class.ToWasmMsg(this)
    }
    
    static load_wasm_from_url(wasm_url, complete, error) {
        function fetch_wasm(wasmfile) {
            let wasm = null;
            function _console_log(chars_ptr, len) {
                let out = "";
                let array = new Uint32Array(wasm.instance.exports.memory.buffer, chars_ptr, len);
                for (let i = 0; i < len; i ++) {
                    out += String.fromCharCode(array[i]);
                }
                console.log(out);
            }
            fetch(wasmfile)
                .then(response => response.arrayBuffer())
                .then(bytes => WebAssembly.instantiate(bytes, {env: {
                _console_log
            }}))
                .then(results => {
                wasm = results;
                complete(wasm);
            }, errors => {
                error(errors);
            });
        }
        fetch_wasm(wasm_url);
    }
}

export class ToWasmMsg {
    constructor(app) {
        this.app = app
        this.ptr = app.new_wasm_msg_with_u64_capacity(1024);
        this.u32_ptr = this.ptr >> 2;
        this.u32_offset = this.u32_ptr + 2;
        
        this.u32_capacity = app.u32[this.u32_ptr] << 1;
    }
    
    reserve_u32(u32_capacity) {
        let app = this.app;
        
        this.u32_capacity += u32_capacity;
        let u64_capacity_needed = ((this.u32_capacity & 1) + this.u32_capacity) >> 1;
        let offset = this.u32_offset - this.u32_ptr;
        let u64_len = ((offset & 1) + offset) >> 1;
        
        if (app.u32[this.u32_ptr] - u64_len < u64_capacity_needed) {
            app.u32[this.u32_ptr + 1] = u64_len;
            this.ptr = this.app.wasm_msg_reserve_u64(this.ptr, u64_capacity_needed);
            this.u32_ptr = this.ptr >> 2;
            this.u32_offset = this.u32_ptr + offset;
        }
    }

    // i forgot how to do memcpy with typed arrays. so, we'll do this.
    push_data_u8(input_buffer) {
        let app = this.app;
        
        let u8_len = input_buffer.byteLength;
        let output_ptr = app.new_to_wasm_data_u8(u8_len);
        
        if ((u8_len & 3) != 0 || (output_ptr & 3) != 0) { // not u32 aligned, do a byte copy
            var u8_out = new Uint8Array(app.memory.buffer, output_ptr, u8_len)
            var u8_in = new Uint8Array(input_buffer)
            for (let i = 0; i < u8_len; i ++) {
                u8_out[i] = u8_in[i];
            }
        }
        else { // do a u32 copy
            let u32_len = u8len >> 2; //4 bytes at a time.
            var u32_out = new Uint32Array(app.memory.buffer, output_ptr, u32_len)
            var u32_in = new Uint32Array(input_buffer)
            for (let i = 0; i < u32_len; i ++) {
                u32_out[i] = u32_in[i];
            }
        }
        
        app.u32[this.u32_offset++] = output_ptr;
        app.u32[this.u32_offset++] = u8_len;
    }
    
    finalise() {
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
        this.u32_capacity = 0;
        
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
    
    destroy() {
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
    
    dispatch() {
        let app = this.app;
        let u32_len = app.u32[this.u32_ptr + 1]<<1;
        while ((this.u32_offset) - this.u32_ptr < u32_len) {
            let msg_id = app.u32[this.u32_offset++];
            this.u32_offset++; // skip second u32 of id
            this.u32_offset++; // skip body len
            // dispatch to deserializer
            this[msg_id]();
            this.u32_offset += this.u32_offset&1; // align
        }
    }
}
