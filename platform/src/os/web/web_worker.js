onmessage = async function(e) {
    let thread_info = e.data;
    
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
            console.error(chars_to_string(chars_ptr, len))
        },
        
        js_console_log: (chars_ptr, len) => {
            console.log(chars_to_string(chars_ptr, len))
        },
    };
    
    WebAssembly.instantiate(thread_info.module, {env}).then(wasm => {
        
        wasm.exports.__stack_pointer.value = thread_info.stack_ptr;
        wasm.exports.__wasm_init_tls(thread_info.tls_ptr);
        wasm.exports.wasm_thread_entrypoint(thread_info.context_ptr);
        
        close();
    }, error => {
        console.error("Cannot instantiate wasm" + error);
    })
}
