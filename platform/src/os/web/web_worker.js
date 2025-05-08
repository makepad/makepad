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
    
    let web_sockets = {}

    let env = {
        memory: thread_info.memory,
        
        js_console_error: (str_ptr, str_len) => {
            console.error(u8_to_string(str_ptr, str_len))
        },
        
        js_console_log: (str_ptr, str_len) => {
            console.log(u8_to_string(str_ptr, str_len))
        },
        
        js_web_socket_send_string(id, str_ptr, str_len){
            let str = u8_to_string(str_ptr, str_len);
            let web_socket = web_sockets[id];
            if(web_socket !== undefined){
                if (web_socket.readyState == 0) {
                    web_socket._queue.push(str)
                }
                else {
                    web_socket.send(str);
                }
            }
        },
        
        js_web_socket_send_binary(id, bin_ptr, bin_len){
            let bin = u8_to_array(bin_ptr, bin_len);
            let web_socket = web_sockets[id];
            if(web_socket !== undefined){
                if (web_socket.readyState == 0) {
                    web_socket._queue.push(bin)
                }
                else {
                    web_socket.send(bin);
                }
            }
        },
        
        js_time_now(){
            return Date.now()/ 1000.0;
        },
        
        js_open_web_socket:(id, url_ptr, url_len)=>{
            let url = u8_to_string(url_ptr, url_len);
            let web_socket = new WebSocket(url);
            web_socket.binaryType = "arraybuffer";
            web_sockets[id] = web_socket;
            
            web_socket.onclose = e => {
                wasm.exports.wasm_web_socket_closed(id);
                delete web_sockets[id];
            }
            web_socket.onerror = e => {
                let err = string_to_u8("" + e);
                wasm.exports.wasm_web_socket_error(id, err.ptr, err.len);
            }
            web_socket.onmessage = e => {
                postMessage("SignalToUI");  // preemptively sends the signal to ui to read from the websocket
                if(typeof e.data == "string"){
                    let data = string_to_u8("" + e.data);
                    wasm.exports.wasm_web_socket_string(id, data.ptr, data.len);
                }
                else{
                    let data = array_to_u8(new Uint8Array(e.data));
                    wasm.exports.wasm_web_socket_binary(id, data.ptr, data.len);
                }
            }
            web_socket.onopen = e => {
                for (let item of web_socket._queue) {
                    web_socket.send(item);
                }
                web_socket._queue.length = 0;
                wasm.exports.wasm_web_socket_opened(id);
            }
            web_socket._queue = []
        }
    };

    function string_to_u8(s){
        const encoder = new TextEncoder();
        const u8_in = encoder.encode(s);
        return array_to_u8(u8_in);
    }

    function u8_to_string(ptr, len){
        let u8 = new Uint8Array(env.memory.buffer, ptr, len);
        let copy = new Uint8Array(len);
        copy.set(u8);
        const decoder = new TextDecoder();
        return decoder.decode(copy);
    }

    function u8_to_array(ptr, len){
        let u8 = new Uint8Array(env.memory.buffer, ptr, len);
        let copy = new Uint8Array(len);
        copy.set(u8);
        return copy
    }

    function array_to_u8(u8_in){
        let u8_out = wasm_new_data_u8(u8_in.length);
        u8_out.array.set(u8_in);
        return u8_out;
    }

    function wasm_new_data_u8(len) {
        let ptr = wasm.exports.wasm_new_data_u8(len);
        return {
            ptr,
            array: new Uint8Array(env.memory.buffer, ptr, len),
            len
        }
    }
    
    let wasm = null;
    const doit = inner_wasm => {
        wasm = inner_wasm;

        if(!thread_info.wasm_bindgen) {
            wasm.exports.__stack_pointer.value = thread_info.stack_ptr;
            wasm.exports.__wasm_init_tls(thread_info.tls_ptr);
        } else {
            wasm.exports.__wbindgen_start();
        }
        if(thread_info.timer > 0){
            this.setInterval(()=>{
                wasm.exports.wasm_thread_timer_entrypoint(thread_info.context_ptr);
            }, thread_info.timer);
        }
        else{
            wasm.exports.wasm_thread_entrypoint(thread_info.context_ptr);
            close();
        }
    };
    if(thread_info.wasm_bindgen) {
        let inner_wasm = await init({module_or_path: thread_info.module, memory: env.memory}, env);
        doit(inner_wasm);
    } else {
        WebAssembly.instantiate(thread_info.module, {env}).then(doit, error => {
            console.error("Cannot instantiate wasm" + error);
        })
    }
}
