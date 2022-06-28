import {FromWasmMsg, ToWasmMsg} from "/makepad/platform/wasm_msg/src/wasm_msg.js"

export class WebGLFromWasmMsg extends FromWasmMsg{
    constructor(wasm){
        super(wasm)
    }
}

export class WasmAppWebGL {
    constructor(canvas, wasm) {
        this.canvas = canvas;
        this.wasm = wasm;
        this.exports = wasm.instance.exports;
        this.memory = wasm.instance.exports.memory;
        
        this.buffer_ref_len_check = 0;
        this.update_array_buffer_refs();
        
        let msg = new FromWasmMsg(this, this.get_wasm_msg_interface());
        let code = msg.read_str();
        msg.destroy();
        
        // this class can also be loaded from file.
        this.ToWasmMsg = new Function("ToWasmMsg","    return class AppToWasmMsg extends ToWasmMsg{" + code + "}")(ToWasmMsg);
        
        console.log(this.ToWasmMsg)
    }
    
    update_array_buffer_refs() {
        if (this.buffer_ref_len_check != this.exports.memory.buffer.byteLength){
            this.f32 = new Float32Array(this.exports.memory.buffer);
            this.u32 = new Uint32Array(this.exports.memory.buffer);
            this.f64 = new Float64Array(this.exports.memory.buffer);
            this.buffer_ref_len_check = this.exports.memory.buffer.byteLength;
        }
    }
    
    get_wasm_msg_interface(){
        let new_ptr = this.exports.get_wasm_msg_interface();
        this.update_array_buffer_refs();
        return new_ptr
    }
    
    new_wasm_msg_with_u64_capacity(capacity){
        let new_ptr = this.exports.new_wasm_msg_with_u64_capacity(capacity)
        this.update_array_buffer_refs();
        return new_ptr
    }

    wasm_msg_reserve_u64(ptr, capacity){
        let new_ptr = this.exports.wasm_msg_reserve_u64(ptr, capacity);
        this.update_array_buffer_refs();
        return new_ptr
    }
    
    wasm_msg_free(ptr){
        this.exports.wasm_msg_free(ptr);
        this.update_array_buffer_refs();
    }
    
    process_to_wasm(msg_ptr){
        let ret_ptr = this.exports.process_to_wasm(msg_ptr)
        this.update_array_buffer_refs();
        return ret_ptr
    }
    
    static create_from_wasm_url(wasm_url, canvas, complete, error) {
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
                complete(new WasmAppWebGL(canvas, wasm));
            }, errors => {
                error(errors);
            });
        }
        fetch_wasm(wasm_url);
    }
}
