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

onmessage = async function (e) {
    let thread_info = e.data;

    async function instantiate_secondary(primary_wasm, env) {
        if (!thread_info.secondary_module) {
            return;
        }
        const secondary_instance = await WebAssembly.instantiate(thread_info.secondary_module, {
            env,
            $p: primary_wasm.exports
        });
        patch_split_table(primary_wasm.exports, secondary_instance.exports);
    }

    function chars_to_string(chars_ptr, len) {
        let out = "";
        let array = new Uint32Array(thread_info.memory.buffer, chars_ptr, len);
        for (let i = 0; i < len; i++) {
            out += String.fromCharCode(array[i]);
        }
        return out
    }

    let web_sockets = {}
    let network_web_sockets = {}
    let network_http_requests = new Map();

    function id_to_key(lo, hi) {
        return `${lo}:${hi}`;
    }

    let env = {
        memory: thread_info.memory,

        js_console_error: (str_ptr, str_len) => {
            console.error(u8_to_string(str_ptr, str_len))
        },

        js_console_log: (str_ptr, str_len) => {
            console.log(u8_to_string(str_ptr, str_len))
        },

        js_web_socket_send_string(id, str_ptr, str_len) {
            let str = u8_to_string(str_ptr, str_len);
            let web_socket = web_sockets[id];
            if (web_socket !== undefined) {
                if (web_socket.readyState == 0) {
                    web_socket._queue.push(str)
                }
                else {
                    web_socket.send(str);
                }
            }
        },

        js_web_socket_send_binary(id, bin_ptr, bin_len) {
            let bin = u8_to_array(bin_ptr, bin_len);
            let web_socket = web_sockets[id];
            if (web_socket !== undefined) {
                if (web_socket.readyState == 0) {
                    web_socket._queue.push(bin)
                }
                else {
                    web_socket.send(bin);
                }
            }
        },

        js_time_now() {
            return Date.now() / 1000.0;
        },

        js_open_web_socket: (id, url_ptr, url_len) => {
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
                if (typeof e.data == "string") {
                    let data = string_to_u8("" + e.data);
                    wasm.exports.wasm_web_socket_string(id, data.ptr, data.len);
                }
                else {
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
        },

        js_network_http_request(
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
        ) {
            let url = u8_to_string(url_ptr, url_len);
            let method = u8_to_string(method_ptr, method_len);
            let headers_raw = u8_to_string(headers_ptr, headers_len);
            let body = body_len > 0 ? u8_to_array(body_ptr, body_len) : undefined;
            let controller = new AbortController();
            let request_key = id_to_key(request_id_lo, request_id_hi);
            network_http_requests.set(request_key, controller);

            let headers = new Headers();
            for (let line of headers_raw.split("\r\n")) {
                if (!line) {
                    continue;
                }
                let sep = line.indexOf(":");
                if (sep <= 0) {
                    continue;
                }
                let key = line.slice(0, sep).trim();
                let value = line.slice(sep + 1).trim();
                if (!key) {
                    continue;
                }
                try {
                    headers.append(key, value);
                }
                catch (_error) {
                }
            }

            fetch(url, {
                method,
                headers,
                body,
                signal: controller.signal,
            }).then(async response => {
                console.log("[makepad][http][req]", method, url);
                let response_headers = "";
                response.headers.forEach((value, key) => {
                    response_headers += `${key}: ${value}\r\n`;
                });
                let response_body = new Uint8Array(await response.arrayBuffer());
                let headers_u8 = string_to_u8(response_headers);
                let body_u8 = array_to_u8(response_body);
                console.log("[makepad][http][res]", response.status, url, response_body.length);
                wasm.exports.wasm_network_http_response(
                    request_id_lo,
                    request_id_hi,
                    metadata_id_lo,
                    metadata_id_hi,
                    response.status,
                    headers_u8.ptr,
                    headers_u8.len,
                    body_u8.ptr,
                    body_u8.len
                );
            }).catch(error => {
                console.error("[makepad][http][err]", method, url, "" + error);
                let message_u8 = string_to_u8("" + error);
                wasm.exports.wasm_network_http_error(
                    request_id_lo,
                    request_id_hi,
                    metadata_id_lo,
                    metadata_id_hi,
                    message_u8.ptr,
                    message_u8.len
                );
            }).finally(() => {
                network_http_requests.delete(request_key);
            });
        },

        js_network_http_cancel(request_id_lo, request_id_hi) {
            let request_key = id_to_key(request_id_lo, request_id_hi);
            let controller = network_http_requests.get(request_key);
            if (controller) {
                controller.abort();
                network_http_requests.delete(request_key);
            }
        },

        js_network_ws_open(socket_id_lo, socket_id_hi, url_ptr, url_len, _headers_ptr, _headers_len) {
            let socket_key = id_to_key(socket_id_lo, socket_id_hi);
            let url = u8_to_string(url_ptr, url_len);
            let web_socket = new WebSocket(url);
            web_socket.binaryType = "arraybuffer";
            network_web_sockets[socket_key] = web_socket;
            web_socket.onclose = _e => {
                wasm.exports.wasm_network_ws_closed(socket_id_lo, socket_id_hi);
                delete network_web_sockets[socket_key];
            };
            web_socket.onerror = e => {
                let message = string_to_u8("" + e);
                wasm.exports.wasm_network_ws_error(
                    socket_id_lo,
                    socket_id_hi,
                    message.ptr,
                    message.len
                );
            };
            web_socket.onmessage = e => {
                if (typeof e.data == "string") {
                    let data = string_to_u8("" + e.data);
                    wasm.exports.wasm_network_ws_text(
                        socket_id_lo,
                        socket_id_hi,
                        data.ptr,
                        data.len
                    );
                }
                else {
                    let data = array_to_u8(new Uint8Array(e.data));
                    wasm.exports.wasm_network_ws_binary(
                        socket_id_lo,
                        socket_id_hi,
                        data.ptr,
                        data.len
                    );
                }
            };
            web_socket.onopen = _e => {
                wasm.exports.wasm_network_ws_opened(socket_id_lo, socket_id_hi);
            };
        },

        js_network_ws_send_binary(socket_id_lo, socket_id_hi, data_ptr, data_len) {
            let socket = network_web_sockets[id_to_key(socket_id_lo, socket_id_hi)];
            if (socket && socket.readyState === WebSocket.OPEN) {
                socket.send(u8_to_array(data_ptr, data_len));
            }
        },

        js_network_ws_send_text(socket_id_lo, socket_id_hi, data_ptr, data_len) {
            let socket = network_web_sockets[id_to_key(socket_id_lo, socket_id_hi)];
            if (socket && socket.readyState === WebSocket.OPEN) {
                socket.send(u8_to_string(data_ptr, data_len));
            }
        },

        js_network_ws_close(socket_id_lo, socket_id_hi) {
            let socket_key = id_to_key(socket_id_lo, socket_id_hi);
            let socket = network_web_sockets[socket_key];
            if (socket) {
                socket.close();
                delete network_web_sockets[socket_key];
            }
        }
    };

    function string_to_u8(s) {
        const encoder = new TextEncoder();
        const u8_in = encoder.encode(s);
        return array_to_u8(u8_in);
    }

    function u8_to_string(ptr, len) {
        let u8 = new Uint8Array(env.memory.buffer, ptr, len);
        let copy = new Uint8Array(len);
        copy.set(u8);
        const decoder = new TextDecoder();
        return decoder.decode(copy);
    }

    function u8_to_array(ptr, len) {
        let u8 = new Uint8Array(env.memory.buffer, ptr, len);
        let copy = new Uint8Array(len);
        copy.set(u8);
        return copy
    }

    function array_to_u8(u8_in) {
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
        return instantiate_secondary(wasm, env).then(() => {
            if (!thread_info.wasm_bindgen) {
                wasm.exports.__stack_pointer.value = thread_info.stack_ptr;
                wasm.exports.__wasm_init_tls(thread_info.tls_ptr);
            } else {
                wasm.exports.__wbindgen_start();
            }
            if (thread_info.timer > 0) {
                this.setInterval(() => {
                    wasm.exports.wasm_thread_timer_entrypoint(thread_info.context_ptr);
                }, thread_info.timer);
            }
            else {
                wasm.exports.wasm_thread_entrypoint(thread_info.context_ptr);
                close();
            }
        });
    };
    if (thread_info.wasm_bindgen) {
        let inner_wasm = await init({ module_or_path: thread_info.module, memory: env.memory }, env);
        await doit(inner_wasm);
    } else {
        WebAssembly.instantiate(thread_info.module, { env }).then(doit, error => {
            console.error("Cannot instantiate wasm" + error);
        })
    }
}
