import { WasmBridge } from "../makepad_wasm_bridge/wasm_bridge.js"

export class WasmWebBrowser extends WasmBridge {
    constructor(wasm, dispatch, canvas) {
        super(wasm, dispatch);
        if (wasm === undefined) {
            return
        }
        /*
        window.onbeforeunload = _ => {
            this.clear_memory_refs();
            for (let worker of this.workers) {
                worker.terminate();
            }
        }*/

        this.wasm_app = this.wasm_create_app();

        this.create_js_message_bridge(this.wasm_app);

        this.dispatch = dispatch;
        this.canvas = canvas;
        this.handlers = {};
        this.timers = [];
        this.text_copy_response = "";
        this.web_sockets = [];
        this.network_web_sockets = {};
        this.network_http_requests = new Map();
        this.window_info = {}
        this.xr_capabilities = {
            vr_supported: false,
            ar_supported: false
        };
        this.xr_supported = false;
        this.signal_timeout = null;
        this.workers = [];
        this.thread_stack_size = 2 * 1024 * 1024;
        this.buffer_upload_serial = 0;
        this.loader_removed = false;
        this.loader_seen_animation_frame = false;
        this.loader_quiet_animation_frames = 0;
        this.loader_fallback_timer = null;
        this.init_detection();
        this.midi_inputs = [];
        this.midi_outputs = [];

        this.dispatch_first_msg();
    }

    install_live_reload_bridge() {
        window.makepad_wasm_live_file_change = (file_name, content) => {
            this.to_wasm.ToWasmLiveFileChange({file_name, content});
            this.do_wasm_pump();
        };

        let queue = window.makepad_wasm_live_file_change_queue || [];
        while (queue.length > 0) {
            let [file_name, content] = queue.shift();
            window.makepad_wasm_live_file_change(file_name, content);
        }
    }

    async load_deps() {
        this.to_wasm = this.new_to_wasm();
        this.install_live_reload_bridge();

        await this.query_xr_capabilities();
        this.update_window_info();

        this.to_wasm.ToWasmInit({
            gpu_info: this.gpu_info,
            cpu_cores: navigator.hardwareConcurrency,
            xr_capabilities: this.xr_capabilities,
            browser_info: {
                protocol: location.protocol + "",
                host: location.host + "",
                hostname: location.hostname + "",
                pathname: location.pathname + "",
                search: location.search + "",
                hash: location.hash + "",
                has_thread_support: this.wasm._has_thread_support
            },
            window_info: this.window_info,
        });

        this.do_wasm_pump();
        // only bind the event handlers now
        // to stop them firing into wasm early
        this.bind_mouse_and_touch();
        this.bind_keyboard();
        this.bind_screen_resize();
        this.focus_keyboard_input();
        this.to_wasm.ToWasmRedrawAll();
        this.start_signal_poll();
        this.do_wasm_pump();
        this.schedule_loader_fallback();
    }

    remove_canvas_loader() {
        if (this.loader_removed) {
            return;
        }
        this.loader_removed = true;
        if (this.loader_fallback_timer) {
            clearTimeout(this.loader_fallback_timer);
            this.loader_fallback_timer = null;
        }
        var loaders = document.getElementsByClassName('canvas_loader');
        while (loaders.length > 0) {
            let loader = loaders[0];
            if (loader.parentNode) {
                loader.parentNode.removeChild(loader);
            }
            else {
                break;
            }
        }
    }

    schedule_loader_fallback() {
        if (this.loader_removed || this.loader_fallback_timer) {
            return;
        }
        this.loader_fallback_timer = window.setTimeout(() => {
            this.remove_canvas_loader();
        }, 1500);
    }

    update_startup_loader(pump_duration_ms) {
        if (this.loader_removed) {
            return;
        }
        this.schedule_loader_fallback();
        if (!this.in_animation_frame) {
            if (pump_duration_ms > 32) {
                this.loader_quiet_animation_frames = 0;
            }
            return;
        }
        this.loader_seen_animation_frame = true;
        if (pump_duration_ms <= 16) {
            this.loader_quiet_animation_frames += 1;
        }
        else {
            this.loader_quiet_animation_frames = 0;
        }
        if (this.loader_seen_animation_frame && this.loader_quiet_animation_frames >= 2) {
            this.remove_canvas_loader();
        }
    }

    FromWasmOpenUrl(args) {
        if (args.in_place) {
            window.location.href = args.url;
        }
        else {
            var link = document.createElement("a");
            link.href = args.url;
            link.target = "_blank";
            link.click();
        }
    }

    FromWasmStartTimer(args) {
        let timer_id = args.timer_id;

        for (let i = 0; i < this.timers.length; i++) {
            if (this.timers[i].timer_id == timer_id) {
                console.error("Timer ID collision!")
                return
            }
        }
        var timer = { timer_id, repeats: args.repeats };
        if (args.repeats === true) {

            timer.sys_id = window.setInterval(e => {
                this.to_wasm.ToWasmTimerFired({ timer_id });
                this.do_wasm_pump();
            }, args.interval * 1000.0);
        }
        else {
            timer.sys_id = window.setTimeout(e => {
                for (let i = 0; i < this.timers.length; i++) {
                    let timer = this.timers[i];
                    if (timer.timer_id == timer_id) {
                        this.timers.splice(i, 1);
                        break;
                    }
                }
                this.to_wasm.ToWasmTimerFired({ timer_id });
                this.do_wasm_pump();
            }, args.interval * 1000.0);
        }
        this.timers.push(timer)
    }

    FromWasmStopTimer(args) {
        for (let i = 0; i < this.timers.length; i++) {
            let timer = this.timers[i];
            if (timer.timer_id == args.timer_id) {
                if (timer.repeats) {
                    window.clearInterval(timer.sys_id);
                }
                else {
                    window.clearTimeout(timer.sys_id);
                }
                this.timers.splice(i, 1);
                return
            }
        }
    }

    FromWasmFullScreen() {
        if (document.body.requestFullscreen) {
            document.body.requestFullscreen();
            return
        }
        if (document.body.webkitRequestFullscreen) {
            document.body.webkitRequestFullscreen();
            return
        }
        if (document.body.mozRequestFullscreen) {
            document.body.mozRequestFullscreen();
            return
        }
    }

    FromWasmNormalScreen() {
        if (this.canvas.exitFullscreen) {
            this.canvas.exitFullscreen();
            return
        }
        if (this.canvas.webkitExitFullscreen) {
            this.canvas.webkitExitFullscreen();
            return
        }
        if (this.canvas.mozExitFullscreen) {
            this.canvas.mozExitFullscreen();
            return
        }
    }

    FromWasmRequestAnimationFrame() {
        if (this.xr !== undefined || this.req_anim_frame_id) {
            return;
        }
        this.req_anim_frame_id = window.requestAnimationFrame(time => {
            if (this.wasm == null) {
                return
            }
            this.req_anim_frame_id = 0;
            if (this.xr !== undefined) {
                return
            }
            this.to_wasm.ToWasmAnimationFrame({ time: time / 1000.0 });
            this.in_animation_frame = true;
            this.do_wasm_pump();
            this.in_animation_frame = false;
        })
    }

    FromWasmSetDocumentTitle(args) {
        document.title = args.title
    }

    FromWasmSetMouseCursor(args) {
        //console.log(args);
        document.body.style.cursor = web_cursor_map[args.web_cursor] || 'default'
    }

    FromWasmTextCopyResponse(args) {
        this.text_copy_response = args.response
    }

    FromWasmShowTextIME(args) {
        this.update_text_area_pos(args);
    }

    FromWasmHideTextIME() {
        this.update_text_area_pos({ x: -3000, y: -3000 });
    }
    /*
    FromWasmWebSocketOpen(args) {
        let id_lo = args.id_lo;
        let id_hi = args.id_hi;
        let url = args.url;
        let web_socket = new WebSocket(args.url);
        web_socket.binaryType = "arraybuffer";
        this.web_sockets[args.web_socket_id] = web_socket;
        
        web_socket.onclose = e => {
            console.log("Auto reconnecting websocket");
            this.to_wasm.ToWasmWebSocketClose({web_socket_id})
            this.do_wasm_pump();
        }
        web_socket.onerror = e => {
            console.error("Websocket error", e);
            this.to_wasm.ToWasmWebSocketError({id_lo,id_hi, error: "" + e})
            this.do_wasm_pump();
        }
        web_socket.onmessage = e => {
            if(typeof e.data == "string"){
                this.to_wasm.ToWasmWebSocketString({
                    id_lo,id_hi,
                    data: e.data
                })
            }
            else{
                this.to_wasm.ToWasmWebSocketBinary({
                    id_lo,id_hi,
                    data: e.data
                })
            }
            this.do_wasm_pump();
        }
        web_socket.onopen = e => {
            for (let item of web_socket._queue) {
                web_socket.send(item);
            }
            web_socket._queue.length = 0;
            this.to_wasm.ToWasmWebSocketOpen({id_lo,id_hi});
            this.do_wasm_pump();
        }
        web_socket._queue = []
    }*/

    FromWasmWebSocketSend(args) {
        let web_socket = this.web_sockets[args.web_socket_id];
        if (web_socket.readyState == 0) {
            web_socket._queue.push(this.clone_data_u8(args.data))
        }
        else {
            web_socket.send(this.clone_data_u8(args.data));
        }
        this.free_data_u8(args.data);
    }

    FromWasmStopAudioOutput(args) {
        if (!this.audio_context) {
            return
        }
        this.audio_context.close();
        this.audio_context = null;
    }

    FromWasmStartAudioOutput(args) {
        if (this.audio_context) {
            return
        }
        const start_worklet = async () => {
            if (this.wasm._secondary_ready) {
                await this.wasm._secondary_ready;
            }
            if (!this.wasm._has_thread_support) {
                console.warn("FromWasmStartAudioOutput skipped: wasm threading support is unavailable");
                return;
            }
            const thread_info = this.alloc_thread_stack(args.context_ptr);
            if (!thread_info) {
                console.warn("FromWasmStartAudioOutput skipped: thread stack allocation prerequisites are unavailable");
                return;
            }

            await this.audio_context.audioWorklet.addModule("./makepad_platform/audio_worklet.js", { credentials: 'omit' });

            const audio_worklet = new AudioWorkletNode(this.audio_context, 'audio-worklet', {
                numberOfInputs: 0,
                numberOfOutputs: 1,
                outputChannelCount: [2],
                processorOptions: { thread_info }
            });

            audio_worklet.port.onmessage = (e) => {
                let data = e.data;
                switch (data.message_type) {
                    case "console_log":
                        console.log(data.value);
                        break;

                    case "console_error":
                        console.error(data.value);
                        break;
                }
            };
            audio_worklet.onprocessorerror = (err) => {
                console.error(err);
            }
            audio_worklet.connect(this.audio_context.destination);

            return audio_worklet;
        };

        let user_interact_hook = (arg) => {
            if (this.audio_context.state === "suspended") {
                this.audio_context.resume();
            }
        }
        this.audio_context = new AudioContext({
            latencyHint: "interactive",
            sampleRate: 48000
        });
        start_worklet().catch(err => console.error(err));
        window.addEventListener('mousedown', user_interact_hook)
        window.addEventListener('touchstart', user_interact_hook)
    }

    FromWasmQueryAudioDevices(args) {
        navigator.mediaDevices?.enumerateDevices().then((devices_enum) => {
            let devices = []
            for (let device of devices_enum) {
                if (device.kind == "audiooutput" || device.kind == "audioinput") {
                    devices.push({
                        web_device_id: "" + device.deviceId,
                        label: "" + device.label,
                        is_output: device.kind == "audiooutput"
                    });
                }
            }
            // safari doesnt report any outputs
            devices.push({
                web_device_id: "",
                label: "",
                is_output: true
            });
            this.to_wasm.ToWasmAudioDeviceList({ devices });
            this.do_wasm_pump();
        })
    }

    FromWasmUseMidiInputs(args) {
        outer:
        for (let input of this.midi_inputs) {
            for (let uid of args.input_uids) {
                if (input.uid == uid) {
                    input.port.onmidimessage = (e) => {
                        let data = e.data;
                        this.to_wasm.ToWasmMidiInputData({
                            uid,
                            data: (data[0] << 16) | (data[1] << 8) | data[2],
                        });
                        this.do_wasm_pump();
                    }
                    continue outer;
                }
            }
            input.onmidimessage = undefined
        }
    }

    FromWasmSendMidiOutput(args) {
        for (let output of this.midi_outputs) {
            if (output.uid == args.uid) {
                output.port.send([(data >> 16) & 0xff, (data >> 8) & 0xff, (data >> 0) & 0xff]);
            }
        }
    }

    FromWasmQueryMidiPorts() {
        if (this.reload_midi_ports) {
            return this.reload_midi_ports();
        }
        if (navigator.requestMIDIAccess) {
            navigator.requestMIDIAccess().then((midi) => {
                this.reload_midi_ports = () => {
                    this.midi_inputs.length = 0;
                    this.midi_outputs.length = 0;
                    let ports = [];
                    for (let input_pair of midi.inputs) {
                        let port = input_pair[1];
                        this.midi_inputs.push({
                            uid: "" + port.id,
                            port
                        });
                        ports.push({
                            uid: "" + port.id,
                            name: port.name,
                            is_output: false
                        });
                    }
                    for (let output_pair of midi.outputs) {
                        let port = output_pair[1];
                        this.midi_outputs.push({
                            uid: "" + port.id,
                            port
                        });
                        ports.push({
                            uid: "" + port.id,
                            name: port.name,
                            is_output: true
                        });
                    }
                    this.to_wasm.ToWasmMidiPortList({ ports });
                    this.do_wasm_pump();
                }
                midi.onstatechange = (e) => {
                    this.reload_midi_ports();
                }
                this.reload_midi_ports();
            }, () => {
                console.error("Cannot open midi");
            });
        }
    }

    FromWasmStartPresentingXR() {

    }

    alloc_thread_stack(context_ptr, timer = 0) {
        if (!this.wasm._has_thread_support) {
            console.warn("alloc_thread_stack unavailable: wasm threading support is disabled");
            return null;
        }
        if (this.exports.__stack_pointer === undefined) {
            console.warn("alloc_thread_stack unavailable: missing __stack_pointer export");
            return null;
        }
        var ret = {
            timer,
            module: this.wasm._module,
            secondary_module: this.wasm._secondary_module,
            memory: this.wasm._memory,
            context_ptr
        };
        if (typeof this.exports.__wbindgen_start !== 'undefined') {
            ret.wasm_bindgen = true;
        } else {
            if (this.exports.__tls_size === undefined) {
                console.warn("alloc_thread_stack unavailable: missing __tls_size export");
                return null;
            }
            if (typeof this.exports.wasm_thread_alloc_tls_and_stack !== "function") {
                console.warn("alloc_thread_stack unavailable: missing wasm_thread_alloc_tls_and_stack export");
                return null;
            }
            let tls_size = this.exports.__tls_size.value;
            tls_size += 8 - (tls_size & 7); // align it to 8 bytes
            let stack_size = this.thread_stack_size; // 8mb
            if ((tls_size + stack_size) & 7 != 0) {
                console.warn("alloc_thread_stack unavailable: stack size is not 8-byte aligned");
                return null;
            }
            ret.tls_ptr = this.exports.wasm_thread_alloc_tls_and_stack((tls_size + stack_size) >> 3);
            this.update_array_buffer_refs();
            ret.stack_ptr = ret.tls_ptr + tls_size + stack_size - 8;
            ret.wasm_bindgen = false;
        }
        return ret;
    }

    // thanks to JP Posma with Zaplib for figuring out how to do the stack_pointer export without wasm bindgen
    // https://github.com/Zaplib/zaplib/blob/650305c856ea64d9c2324cbd4b8751ffbb971ac3/zaplib/cargo-zaplib/src/build.rs#L48
    // https://github.com/Zaplib/zaplib/blob/7cb3bead16f963e60c840aa2be3bf28a47ac533e/zaplib/web/common.ts#L313
    // And Ingvar Stepanyan for https://web.dev/webassembly-threads/
    // example build command:
    // RUSTFLAGS="-C target-feature=+atomics,+bulk-memory,+mutable-globals -C link-arg=--export=__stack_pointer" cargo build -p thing_to_compile --target=wasm32-unknown-unknown -Z build-std=panic_abort,std
    FromWasmCreateThread(args) {
        (async () => {
            if (this.wasm._secondary_ready) {
                await this.wasm._secondary_ready;
            }
            if (!this.wasm._has_thread_support) {
                console.error("FromWasmCreateThread not available, wasm file not compiled with threading support");
                return;
            }
            let thread_info = this.alloc_thread_stack(args.context_ptr, args.timer);
            if (!thread_info) {
                console.error("FromWasmCreateThread not available, thread stack allocation prerequisites are missing");
                return;
            }
            let worker = new Worker(
                './makepad_platform/web_worker.js',
                { type: 'module' }
            );
            worker.postMessage(thread_info);

            this.workers.push(worker);
        })().catch(err => console.error(err));
    }

    start_signal_poll() {
        this.poll_timer = window.setInterval(e => {
            let flags = this.exports.wasm_check_signal();
            if (flags != 0) {
                this.to_wasm.ToWasmSignal({ flags });
                this.do_wasm_pump();
            }
        }, 0.016 * 1000.0);
    }

    parse_and_set_headers(request, headers_string) {
        let lines = headers_string.split("\r\n");
        for (let line of lines) {
            let parts = line.split(": ");
            if (parts.length == 2) {
                request.setRequestHeader(parts[0], parts[1]);
            }
        }
    }

    id_to_key(id_lo, id_hi) {
        return `${id_lo}:${id_hi}`;
    }

    alloc_u8(input_u8) {
        let ptr = this.wasm_new_data_u8(input_u8.length);
        let out = new Uint8Array(this.memory.buffer, ptr, input_u8.length);
        out.set(input_u8);
        return { ptr, len: input_u8.length };
    }

    string_to_u8(s) {
        const encoder = new TextEncoder();
        return this.alloc_u8(encoder.encode(s));
    }

    array_to_u8(u8_array) {
        return this.alloc_u8(u8_array);
    }

    u8_to_array(ptr, len) {
        let u8 = new Uint8Array(this.memory.buffer, ptr, len);
        let copy = new Uint8Array(len);
        copy.set(u8);
        return copy;
    }

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
        let url = this.u8_to_string(url_ptr, url_len);
        let method = this.u8_to_string(method_ptr, method_len);
        let headers_raw = this.u8_to_string(headers_ptr, headers_len);
        let body = body_len > 0 ? this.u8_to_array(body_ptr, body_len) : undefined;
        let controller = new AbortController();
        let request_key = this.id_to_key(request_id_lo, request_id_hi);
        this.network_http_requests.set(request_key, controller);

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

        console.log("[makepad][http][req]", method, url);
        fetch(url, {
            method,
            headers,
            body,
            signal: controller.signal,
        }).then(async response => {
            let response_headers = "";
            response.headers.forEach((value, key) => {
                response_headers += `${key}: ${value}\r\n`;
            });
            let response_body = new Uint8Array(await response.arrayBuffer());
            let headers_u8 = this.string_to_u8(response_headers);
            let body_u8 = this.array_to_u8(response_body);
            console.log("[makepad][http][res]", response.status, url, response_body.length);
            this.exports.wasm_network_http_response(
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
            let message_u8 = this.string_to_u8("" + error);
            this.exports.wasm_network_http_error(
                request_id_lo,
                request_id_hi,
                metadata_id_lo,
                metadata_id_hi,
                message_u8.ptr,
                message_u8.len
            );
        }).finally(() => {
            this.network_http_requests.delete(request_key);
        });
    }

    js_network_http_cancel(request_id_lo, request_id_hi) {
        let request_key = this.id_to_key(request_id_lo, request_id_hi);
        let controller = this.network_http_requests.get(request_key);
        if (controller) {
            controller.abort();
            this.network_http_requests.delete(request_key);
        }
    }

    js_network_ws_open(socket_id_lo, socket_id_hi, url_ptr, url_len, _headers_ptr, _headers_len) {
        let socket_key = this.id_to_key(socket_id_lo, socket_id_hi);
        let url = this.u8_to_string(url_ptr, url_len);
        let web_socket = new WebSocket(url);
        web_socket.binaryType = "arraybuffer";
        this.network_web_sockets[socket_key] = web_socket;
        web_socket.onclose = _e => {
            this.exports.wasm_network_ws_closed(socket_id_lo, socket_id_hi);
            delete this.network_web_sockets[socket_key];
        };
        web_socket.onerror = e => {
            let message = this.string_to_u8("" + e);
            this.exports.wasm_network_ws_error(
                socket_id_lo,
                socket_id_hi,
                message.ptr,
                message.len
            );
        };
        web_socket.onmessage = e => {
            if (typeof e.data == "string") {
                let data = this.string_to_u8("" + e.data);
                this.exports.wasm_network_ws_text(
                    socket_id_lo,
                    socket_id_hi,
                    data.ptr,
                    data.len
                );
            }
            else {
                let data = this.array_to_u8(new Uint8Array(e.data));
                this.exports.wasm_network_ws_binary(
                    socket_id_lo,
                    socket_id_hi,
                    data.ptr,
                    data.len
                );
            }
        };
        web_socket.onopen = _e => {
            this.exports.wasm_network_ws_opened(socket_id_lo, socket_id_hi);
        };
    }

    js_network_ws_send_binary(socket_id_lo, socket_id_hi, data_ptr, data_len) {
        let socket = this.network_web_sockets[this.id_to_key(socket_id_lo, socket_id_hi)];
        if (socket && socket.readyState === WebSocket.OPEN) {
            socket.send(this.u8_to_array(data_ptr, data_len));
        }
    }

    js_network_ws_send_text(socket_id_lo, socket_id_hi, data_ptr, data_len) {
        let socket = this.network_web_sockets[this.id_to_key(socket_id_lo, socket_id_hi)];
        if (socket && socket.readyState === WebSocket.OPEN) {
            socket.send(this.u8_to_string(data_ptr, data_len));
        }
    }

    js_network_ws_close(socket_id_lo, socket_id_hi) {
        let socket_key = this.id_to_key(socket_id_lo, socket_id_hi);
        let socket = this.network_web_sockets[socket_key];
        if (socket) {
            socket.close();
            delete this.network_web_sockets[socket_key];
        }
    }

    FromWasmHTTPRequest(args) {
        const req = new XMLHttpRequest();
        req.open(args.method, args.url);
        req.responseType = "arraybuffer";
        this.parse_and_set_headers(req, args.headers);

        // TODO decode in appropiate format
        const decoder = new TextDecoder('UTF-8', { fatal: true });
        let body = decoder.decode(this.clone_data_u8(args.body));

        req.addEventListener("load", event => {
            let responseEvent = event.target;
            if (responseEvent.status < 200 || responseEvent.status >= 300) {
                report_browser_issue("xhr.http_error", {
                    method: args.method,
                    url: args.url,
                    status: responseEvent.status,
                });
            }

            this.to_wasm.ToWasmHTTPResponse({
                request_id_lo: args.request_id_lo,
                request_id_hi: args.request_id_hi,
                metadata_id_lo: args.metadata_id_lo,
                metadata_id_hi: args.metadata_id_hi,
                status: responseEvent.status,
                body: responseEvent.response,
                headers: responseEvent.getAllResponseHeaders()
            });
            this.do_wasm_pump();
        });

        req.addEventListener("error", event => {
            let errorMessage = "An error occurred with the HTTP request.";
            if (!navigator.onLine) {
                errorMessage = "The browser is offline.";
            }
            report_browser_issue("xhr.error", {
                method: args.method,
                url: args.url,
                message: errorMessage,
            });

            this.to_wasm.ToWasmHttpRequestError({
                request_id_lo: args.request_id_lo,
                request_id_hi: args.request_id_hi,
                error: errorMessage,
            });
            this.do_wasm_pump();
        });

        req.addEventListener("timeout", event => {
            report_browser_issue("xhr.timeout", {
                method: args.method,
                url: args.url,
            });
            this.to_wasm.ToWasmHttpRequestError({
                request_id_lo: args.request_id_lo,
                request_id_hi: args.request_id_hi,
                error: "The HTTP request timed out.",
            });
            this.do_wasm_pump();
        });

        req.addEventListener("abort", event => {
            report_browser_issue("xhr.abort", {
                method: args.method,
                url: args.url,
            });
            this.to_wasm.ToWasmHttpRequestError({
                request_id_lo: args.request_id_lo,
                request_id_hi: args.request_id_hi,
                error: "The HTTP request was aborted.",
            });
            this.do_wasm_pump();
        });

        req.addEventListener("progress", event => {
            if (event.lengthComputable) {
                this.to_wasm.ToWasmHttpResponseProgress({
                    request_id_lo: args.request_id_lo,
                    request_id_hi: args.request_id_hi,
                    loaded: event.loaded,
                    total: event.total,
                });
                this.do_wasm_pump();
            }
        });

        req.upload.addEventListener("progress", (event) => {
            if (event.lengthComputable) {
                this.to_wasm.ToWasmHttpUploadProgress({
                    request_id_lo: args.request_id_lo,
                    request_id_hi: args.request_id_hi,
                    loaded: event.loaded,
                    total: event.total,
                });
                this.do_wasm_pump();
            }
        });

        req.send(body);
        this.free_data_u8(args.body);
    }

    FromWasmCancelHTTPRequest(args) {
        // Web doesn't provide a way to cancel XHR requests by ID
        // This would require tracking requests, which we don't currently do
    }

    async FromWasmCheckPermission(args) {
        try {
            if (args.permission === 'microphone' || args.permission === 'camera') {
                // Check if Permissions API is available
                if (navigator.permissions && navigator.permissions.query) {
                    const result = await navigator.permissions.query({ name: args.permission });
                    let status;
                    switch (result.state) {
                        case 'granted':
                            status = 1; // Granted
                            break;
                        case 'denied':
                            status = 3; // DeniedPermanent (browsers don't distinguish)
                            break;
                        case 'prompt':
                        default:
                            status = 0; // NotDetermined
                            break;
                    }
                    this.to_wasm.ToWasmPermissionResult({
                        permission: args.permission,
                        request_id: args.request_id,
                        status: status
                    });
                } else {
                    // Fallback: try to check if we already have a stream
                    const kind = args.permission === 'microphone' ? 'audioinput' : 'videoinput';
                    try {
                        const devices = await navigator.mediaDevices.enumerateDevices();
                        const hasDevice = devices.some(device => device.kind === kind && device.label !== '');
                        this.to_wasm.ToWasmPermissionResult({
                            permission: args.permission,
                            request_id: args.request_id,
                            status: hasDevice ? 1 : 0 // Granted if we see labels, NotDetermined otherwise
                        });
                    } catch {
                        // Can't determine, assume not determined
                        this.to_wasm.ToWasmPermissionResult({
                            permission: args.permission,
                            request_id: args.request_id,
                            status: 0 // NotDetermined
                        });
                    }
                }
            } else {
                // Unknown permission type
                this.to_wasm.ToWasmPermissionResult({
                    permission: args.permission,
                    request_id: args.request_id,
                    status: 3 // DeniedPermanent
                });
            }
        } catch (error) {
            console.error('Permission check failed:', error);
            this.to_wasm.ToWasmPermissionResult({
                permission: args.permission,
                request_id: args.request_id,
                status: 3 // DeniedPermanent on error
            });
        }
        this.do_wasm_pump();
    }

    async FromWasmRequestPermission(args) {
        try {
            if (args.permission === 'microphone' || args.permission === 'camera') {
                try {
                    // Request media access
                    const constraints = args.permission === 'microphone' ? { audio: true } : { video: true };
                    const stream = await navigator.mediaDevices.getUserMedia(constraints);
                    // Successfully got permission, close the stream immediately
                    stream.getTracks().forEach(track => track.stop());

                    this.to_wasm.ToWasmPermissionResult({
                        permission: args.permission,
                        request_id: args.request_id,
                        status: 1 // Granted
                    });
                } catch (error) {
                    // Permission was denied or error occurred
                    let status = 3; // DeniedPermanent (default)

                    if (error.name === 'NotAllowedError' || error.name === 'PermissionDeniedError') {
                        // User explicitly denied permission
                        status = 3; // DeniedPermanent (browsers don't re-prompt)
                    } else if (error.name === 'NotFoundError' || error.name === 'DevicesNotFoundError') {
                        // No device found
                        status = 3; // DeniedPermanent (can't grant without device)
                    } else if (error.name === 'NotReadableError' || error.name === 'TrackStartError') {
                        // Device is in use or hardware error
                        status = 2; // DeniedCanRetry
                    }

                    this.to_wasm.ToWasmPermissionResult({
                        permission: args.permission,
                        request_id: args.request_id,
                        status: status
                    });
                }
            } else {
                // Unknown permission type
                this.to_wasm.ToWasmPermissionResult({
                    permission: args.permission,
                    request_id: args.request_id,
                    status: 3 // DeniedPermanent
                });
            }
        } catch (error) {
            console.error('Permission request failed:', error);
            this.to_wasm.ToWasmPermissionResult({
                permission: args.permission,
                request_id: args.request_id,
                status: 3 // DeniedPermanent on error
            });
        }
        this.do_wasm_pump();
    }

    // calling into wasm


    wasm_terminate_thread_pools() {
        this.exports.wasm_terminate_thread_pools(this.wasm_app);
    }

    wasm_create_app() {
        let new_ptr = this.exports.wasm_create_app();
        this.update_array_buffer_refs();
        return new_ptr
    }


    wasm_return_first_msg() {
        let ret_ptr = this.exports.wasm_return_first_msg(this.wasm_app)
        this.update_array_buffer_refs();
        return this.new_from_wasm(ret_ptr);
    }

    dispatch_first_msg() {
        let from_wasm = this.wasm_return_first_msg();
        from_wasm.dispatch_on_app();
        from_wasm.free();
    }

    do_wasm_pump() {
        let started = performance.now();
        this.buffer_upload_serial += 1;
        let to_wasm = this.to_wasm;
        this.to_wasm = this.new_to_wasm();
        let from_wasm = this.wasm_process_msg(to_wasm);
        from_wasm.dispatch_on_app();
        from_wasm.free();
        this.update_startup_loader(performance.now() - started);
    }


    wasm_process_msg(to_wasm) {
        if (this.debug_sum_ptr !== undefined) {
            let ptr = this.debug_sum_ptr;
            this.debug_sum_ptr = undefined;
            var u8_out = new Uint8Array(this.memory.buffer, ptr.ptr, ptr.len);
            let sum = 0
            for (let i = 0; i < ptr.len; i++) {
                sum += u8_out[i];
            }
        }


        let ret_ptr = this.exports.wasm_process_msg(to_wasm.release_ownership(), this.wasm_app)
        this.update_array_buffer_refs();
        return this.new_from_wasm(ret_ptr);
    }


    // init and setup


    init_detection() {
        this.detect = {
            user_agent: window.navigator.userAgent,
            is_mobile_safari: window.navigator.platform.match(/iPhone|iPad/i),
            is_touch_device: ('ontouchstart' in window || navigator.maxTouchPoints),
            is_firefox: navigator.userAgent.toLowerCase().indexOf('firefox') > -1,
            use_touch_scroll_overlay: window.ontouchstart === null,
        };

        this.detect.is_android = this.detect.user_agent.match(/Android/i)
        this.detect.is_add_to_homescreen_safari = this.is_mobile_safari && navigator.standalone
    }

    update_window_info() {
        var dpi_factor = window.devicePixelRatio;
        var w;
        var h;
        var canvas = this.canvas;

        if (canvas.getAttribute("fullpage")) {
            if (this.detect.is_add_to_homescreen_safari) { // extremely ugly. but whatever.
                if (window.orientation == 90 || window.orientation == -90) {
                    h = screen.width;
                    w = screen.height - 90;
                }
                else {
                    w = screen.width;
                    h = screen.height - 80;
                }
            }
            else {
                w = window.innerWidth;
                h = window.innerHeight;
            }
        }
        else {
            w = canvas.offsetWidth;
            h = canvas.offsetHeight;
        }
        var sw = canvas.width = w * dpi_factor;
        var sh = canvas.height = h * dpi_factor;

        this.gl.viewport(0, 0, sw, sh);

        this.window_info.dpi_factor = dpi_factor;
        this.window_info.inner_width = canvas.offsetWidth;
        this.window_info.inner_height = canvas.offsetHeight;
        this.window_info.is_fullscreen = is_fullscreen();
        this.window_info.can_fullscreen = can_fullscreen();
    }

    query_xr_capabilities() {
        return Promise.all([]);
    }

    bind_screen_resize() {
        this.handlers.on_screen_resize = () => {
            this.update_window_info();
            if (this.to_wasm !== undefined) {
                this.to_wasm.ToWasmResizeWindow({ window_info: this.window_info });
                this.FromWasmRequestAnimationFrame();
            }
        }

        // TODO! BIND THESE SOMEWHERE USEFUL
        this.handlers.on_app_got_focus = () => {
            this.to_wasm.ToWasmWindowGotFocus();
            this.do_wasm_pump();
        }

        this.handlers.on_app_lost_focus = () => {
            this.to_wasm.ToWasmWindowLostFocus();
            this.do_wasm_pump();
        }

        window.addEventListener('resize', _ => this.handlers.on_screen_resize())
        window.addEventListener('orientationchange', _ => this.handlers.on_screen_resize())
    }

    bind_mouse_and_touch() {

        var canvas = this.canvas
        /*
        TODO fix/test this
        let overlay_scroll_pointer;
        if (this.detect.use_touch_scroll_overlay) {
            var ts = this.touch_scroll_overlay = document.createElement('div')
            ts.className = "makepad_webgl_scroll_overlay"
            var ts_inner = document.createElement('div')
            var style = document.createElement('style')
            style.innerHTML = "\n"
                + "div.makepad_webgl_scroll_overlay {\n"
                + "z-index: 10000;\n"
                + "margin:0;\n"
                + "overflow:scroll;\n"
                + "top:0;\n"
                + "left:0;\n"
                + "width:100%;\n"
                + "height:100%;\n"
                + "position:fixed;\n"
                + "background-color:transparent\n"
                + "}\n"
                + "div.cx_webgl_scroll_overlay div{\n"
                + "margin:0;\n"
                + "width:400000px;\n"
                + "height:400000px;\n"
                + "background-color:transparent\n"
                + "}\n"
          
            document.body.appendChild(style)
            ts.appendChild(ts_inner);
            document.body.appendChild(ts);
            canvas = ts;
          
            ts.scrollTop = 200000;
            ts.scrollLeft = 200000;
            let last_scroll_top = ts.scrollTop;
            let last_scroll_left = ts.scrollLeft;
            let scroll_timeout = null;
          
            this.handlers.on_overlay_scroll = e => {
                let new_scroll_top = ts.scrollTop;
                let new_scroll_left = ts.scrollLeft;
                let dx = new_scroll_left - last_scroll_left;
                let dy = new_scroll_top - last_scroll_top;
                last_scroll_top = new_scroll_top;
                last_scroll_left = new_scroll_left;
              
                window.clearTimeout(scroll_timeout);
              
                scroll_timeout = window.setTimeout(_ => {
                    ts.scrollTop = 200000;
                    ts.scrollLeft = 200000;
                    last_scroll_top = ts.scrollTop;
                    last_scroll_left = ts.scrollLeft;
                }, 200);
              
                let finger = overlay_scroll_pointer;
                if (overlay_scroll_pointer) {
                    this.to_wasm.ToWasmScroll({
                        x: overlay_scroll_pointer.x,
                        y: overlay_scroll_pointer.y,
                        modifiers: overlay_scroll_pointer.modifiers,
                        is_touch: overlay_scroll_pointer.is_touch,
                        scroll_x: dx,
                        scroll_y: dy,
                        time: e.timeStamp / 1000.0;
                    });
                    this.do_wasm_pump();
                }
            }
          
            ts.addEventListener('scroll', e => this.handlers.on_overlay_scroll(e))
        }*/

        /*
        var mouse_fingers = [];
        function mouse_to_finger(e) {
            let mf = mouse_fingers[e.button] || (mouse_fingers[e.button] = {});
            mf.x = e.pageX;
            mf.y = e.pageY;
            mf.digit = e.button;
            mf.time = e.timeStamp / 1000.0;
            mf.modifiers = pack_key_modifier(e);
            mf.touch = false;
            return mf
        }*/

        function mouse_to_wasm_wmouse(e) {
            return {
                x: e.pageX,
                y: e.pageY,
                button: e.button,
                time: e.timeStamp / 1000.0,
                modifiers: pack_key_modifier(e)
            }
        }
        //let current_mouse_down = null;
        this.handlers.on_mouse_down = e => {
            e.preventDefault();
            this.focus_keyboard_input();
            //if (current_mouse_down === null || current_mouse_down === e.button){
            //    current_mouse_down = e.button;
            this.to_wasm.ToWasmMouseDown({ mouse: mouse_to_wasm_wmouse(e) });
            this.do_wasm_pump();
            //}
        }

        this.handlers.on_mouse_up = e => {
            e.preventDefault();
            //if (current_mouse_down == e.button){
            //    current_mouse_down = null;
            this.to_wasm.ToWasmMouseUp({ mouse: mouse_to_wasm_wmouse(e) });
            this.do_wasm_pump();
            //}
        }

        this.handlers.on_mouse_move = e => {
            document.body.scrollTop = 0;
            document.body.scrollLeft = 0;
            this.to_wasm.ToWasmMouseMove({ was_out: false, mouse: mouse_to_wasm_wmouse(e) });
            this.do_wasm_pump();
        }

        this.handlers.on_mouse_out = e => {
            this.to_wasm.ToWasmMouseMove({ was_out: true, mouse: mouse_to_wasm_wmouse(e) });
            this.do_wasm_pump();
        }

        canvas.addEventListener('mousedown', e => this.handlers.on_mouse_down(e))
        window.addEventListener('mouseup', e => this.handlers.on_mouse_up(e))
        window.addEventListener('mousemove', e => this.handlers.on_mouse_move(e));
        window.addEventListener('mouseout', e => this.handlers.on_mouse_out(e));

        this.handlers.on_contextmenu = e => {
            e.preventDefault()
            return false
        }

        canvas.addEventListener('contextmenu', e => this.handlers.on_contextmenu(e))

        function touch_to_wasm_wtouch(t, state) {
            return {
                state,
                x: t.pageX,
                y: t.pageY,
                radius_x: t.radiusX,
                radius_y: t.radiusY,
                rotation_angle: t.rotationAngle,
                force: t.force,
                uid: t.identifier === undefined ? i : t.identifier,
            }
        }

        function touches_to_wasm_wtouches(e, state) {
            let f = [];

            for (let i = 0; i < e.changedTouches.length; i++) {
                f.push(touch_to_wasm_wtouch(e.changedTouches[i], state));
            }

            touch_loop:
            for (let i = 0; i < e.touches.length; i++) {
                let t = e.touches[i];
                for (let j = 0; j < e.changedTouches.length; j++) {
                    if (e.changedTouches[j].identifier == t.identifier) {
                        continue touch_loop;
                    }
                }
                f.push(touch_to_wasm_wtouch(t, 0));
            }
            /*
            let dump = "";
            let statev = ["stable","start","move","stop"]
            for( let i = 0; i<f.length;i++){
                dump += statev[f[i].state] +"("+(-f[i].uid%10)+"), "
            }
            console.log(dump);*/
            return f
        }

        this.handlers.on_touchstart = e => {
            e.preventDefault()
            this.to_wasm.ToWasmTouchUpdate({
                time: e.timeStamp / 1000.0,
                modifiers: pack_key_modifier(e),
                touches: touches_to_wasm_wtouches(e, 1)
            });
            this.do_wasm_pump();
            return false
        }

        this.handlers.on_touchmove = e => {
            e.preventDefault();
            this.to_wasm.ToWasmTouchUpdate({
                time: e.timeStamp / 1000.0,
                modifiers: pack_key_modifier(e),
                touches: touches_to_wasm_wtouches(e, 2)
            });
            this.do_wasm_pump();
            return false
        }

        this.handlers.on_touch_end_cancel_leave = e => {
            e.preventDefault();
            this.to_wasm.ToWasmTouchUpdate({
                time: e.timeStamp / 1000.0,
                modifiers: pack_key_modifier(e),
                touches: touches_to_wasm_wtouches(e, 3)
            });
            this.do_wasm_pump();
            return false
        }

        canvas.addEventListener('touchstart', e => this.handlers.on_touchstart(e))
        canvas.addEventListener('touchmove', e => this.handlers.on_touchmove(e), { passive: false })
        canvas.addEventListener('touchend', e => this.handlers.on_touch_end_cancel_leave(e));
        canvas.addEventListener('touchcancel', e => this.handlers.on_touch_end_cancel_leave(e));
        canvas.addEventListener('touchleave', e => this.handlers.on_touch_end_cancel_leave(e));

        var last_wheel_time;
        var last_was_wheel;
        this.handlers.on_mouse_wheel = e => {
            //var finger = mouse_to_finger(e)
            e.preventDefault()
            let delta = e.timeStamp - last_wheel_time;
            last_wheel_time = e.timeStamp;
            // typical web bullshit. this reliably detects mousewheel or touchpad on mac in safari
            if (this.detect.is_firefox) {
                last_was_wheel = e.deltaMode == 1
            }
            else { // detect it
                if (Math.abs(Math.abs((e.deltaY / e.wheelDeltaY)) - (1. / 3.)) < 0.00001 || !last_was_wheel && delta < 250) {
                    last_was_wheel = false;
                }
                else {
                    last_was_wheel = true;
                }
            }
            //console.log(e.deltaY / e.wheelDeltaY);
            //last_delta = delta;
            var fac = 1
            if (e.deltaMode === 1) fac = 40
            else if (e.deltaMode === 2) fac = window.offsetHeight

            this.to_wasm.ToWasmScroll({
                x: e.pageX,
                y: e.pageY,
                modifiers: pack_key_modifier(e),
                is_touch: !last_was_wheel,
                scroll_x: e.deltaX * fac,
                scroll_y: e.deltaY * fac,
                time: e.timeStamp / 1000.0,
            });
            this.do_wasm_pump();
        };
        canvas.addEventListener('wheel', e => this.handlers.on_mouse_wheel(e))
    }

    bind_keyboard() {
        if (this.detect.is_mobile_safari || this.detect.is_android) { // mobile keyboards are unusable on a UI like this. Not happening.
            return
        }

        var ta = this.text_area = document.createElement('textarea')
        ta.className = "cx_webgl_textinput"
        ta.setAttribute('autocomplete', 'off')
        ta.setAttribute('autocorrect', 'off')
        ta.setAttribute('autocapitalize', 'off')
        ta.setAttribute('spellcheck', 'false')
        var style = document.createElement('style')

        style.innerHTML = "\n"
            + "textarea.cx_webgl_textinput {\n"
            + "z-index: 1000;\n"
            + "position: absolute;\n"
            + "opacity: 0;\n"
            + "border-radius: 4px;\n"
            + "color:white;\n"
            + "font-size: 6;\n"
            + "background: gray;\n"
            + "-moz-appearance: none;\n"
            + "appearance:none;\n"
            + "border:none;\n"
            + "resize: none;\n"
            + "outline: none;\n"
            + "overflow: hidden;\n"
            + "text-indent: 0px;\n"
            + "padding: 0 0px;\n"
            + "margin: 0 -1px;\n"
            + "text-indent: 0px;\n"
            + "-ms-user-select: text;\n"
            + "-moz-user-select: text;\n"
            + "-webkit-user-select: text;\n"
            + "user-select: text;\n"
            + "white-space: pre!important;\n"
            + "}\n"
            + "textarea: focus.cx_webgl_textinput {\n"
            + "outline: 0px !important;\n"
            + "-webkit-appearance: none;\n"
            + "}"

        document.body.appendChild(style)
        ta.style.left = -100 + 'px'
        ta.style.top = -100 + 'px'
        ta.style.height = 1 + 'px'
        ta.style.width = 1 + 'px'

        //document.addEventListener('focusout', this.onFocusOut.bind(this))
        var was_paste = false;
        this.neutralize_ime = false;
        var last_len = 0;

        this.handlers.on_cut = e => {
            setTimeout(_ => {
                ta.value = "";
                last_len = 0;
            }, 0)
        }

        ta.addEventListener('cut', e => this.handlers.on_cut(e));

        this.handlers.on_copy = e => {
            setTimeout(_ => {
                ta.value = "";
                last_len = 0;
            }, 0)
        }

        ta.addEventListener('copy', e => this.handlers.on_copy(e));

        this.handlers.on_paste = e => {
            was_paste = true;
        }

        ta.addEventListener('paste', e => this.handlers.on_paste(e));

        this.handlers.on_select = e => { }

        ta.addEventListener('select', e => this.handlers.on_select(e))

        this.handlers.on_input = e => {
            // if IME composition is in progress, do not handle the normal input event
            if (is_composing) {
                // console.log('⏸️ Skipping input event during composition');
                return;
            }
            if (ta.value.length > 0) {
                if (was_paste) {
                    was_paste = false;

                    this.to_wasm.ToWasmTextInput({
                        was_paste: true,
                        input: ta.value.substring(last_len),
                        replace_last: false,
                    })
                    ta.value = "";
                }
                else {
                    var replace_last = false;
                    var text_value = ta.value;
                    if (ta.value.length >= 2) { // we want the second char
                        text_value = ta.value.substring(1, 2);
                        ta.value = text_value;
                    }
                    else if (ta.value.length == 1 && last_len == ta.value.length) { // its an IME replace
                        replace_last = true;
                    }
                    // we should send a replace last
                    if (replace_last || text_value != '\n') {
                        this.to_wasm.ToWasmTextInput({
                            was_paste: false,
                            input: text_value,
                            replace_last: replace_last,
                        });
                    }
                }
                this.do_wasm_pump();
            }
            last_len = ta.value.length;
        };
        ta.addEventListener('input', e => this.handlers.on_input(e));

        // add composition events handling, this is the standard way to handle IME input
        var is_composing = false;
        var composition_data = "";

        ta.addEventListener('compositionstart', e => {
            is_composing = true;
            composition_data = "";
        });

        ta.addEventListener('compositionupdate', e => {
            composition_data = e.data || "";
        });

        ta.addEventListener('compositionend', e => {
            is_composing = false;

            // send final IME input result
            if (e.data && e.data !== '\n') {
                this.to_wasm.ToWasmTextInput({
                    was_paste: false,
                    input: e.data,
                    replace_last: composition_data.length > 0, // 如果之前有组合数据，则替换
                });
                this.do_wasm_pump();
            }

            composition_data = "";
            // clear textarea
            ta.value = "";
            last_len = 0;
        });

        ta.addEventListener('mousedown', e => this.handlers.on_mouse_down(e));
        ta.addEventListener('mouseup', e => this.handlers.on_mouse_up(e));
        ta.addEventListener('wheel', e => this.handlers.on_mouse_wheel(e));

        ta.addEventListener('contextmenu', e => this.handlers.on_contextmenu(e));

        ta.addEventListener('blur', e => {
            this.focus_keyboard_input();
        })

        var ugly_ime_hack = false;

        this.handlers.on_keydown = e => {
            let code = e.keyCode;

            //if (code == 91) {firefox_logo_key = true; e.preventDefault();}
            if (code == 18 || code == 17 || code == 16) e.preventDefault(); // alt
            if (code === 8 || code === 9) e.preventDefault() // backspace/tab
            if ((code === 88 || code == 67) && (e.metaKey || e.ctrlKey)) { // copy or cut
                // we need to request the clipboard
                this.to_wasm.ToWasmTextCopy();
                this.do_wasm_pump();
                ta.value = this.text_copy_response;
                ta.selectionStart = 0;
                ta.selectionEnd = ta.value.length;
            }
            //    this.keyboardCut = true // x cut
            //if(code === 65 && (e.metaKey || e.ctrlKey)) this.keyboardSelectAll = true     // all (select all)
            if (code === 89 && (e.metaKey || e.ctrlKey)) e.preventDefault() // all (select all)
            if (code === 83 && (e.metaKey || e.ctrlKey)) e.preventDefault() // ctrl s
            if (code === 90 && (e.metaKey || e.ctrlKey)) {
                this.update_text_area_pos();
                ta.value = "";
                ugly_ime_hack = true;
                ta.readOnly = true;
                e.preventDefault()
            }
            // if we are using arrow keys, home or end
            let key_code = e.keyCode;

            if (key_code >= 33 && key_code <= 40) {
                ta.value = "";
                last_len = ta.value.length;
            }
            //if(key_code
            this.to_wasm.ToWasmKeyDown({
                key: {
                    key_code: key_code,
                    char_code: e.charCode,
                    is_repeat: e.repeat,
                    time: e.timeStamp / 1000.0,
                    modifiers: pack_key_modifier(e)
                }
            })

            this.do_wasm_pump();
        };

        ta.addEventListener('keydown', e => this.handlers.on_keydown(e));

        this.handlers.on_keyup = e => {
            let code = e.keyCode;

            if (code == 18 || code == 17 || code == 16) e.preventDefault(); // alt
            if (code == 91) { e.preventDefault(); }
            var ta = this.text_area;
            if (ugly_ime_hack) {
                ugly_ime_hack = false;
                document.body.removeChild(ta);
                this.bind_keyboard();
                this.update_text_area_pos();
            }
            this.to_wasm.ToWasmKeyUp({
                key: {
                    key_code: e.keyCode,
                    char_code: e.charCode,
                    is_repeat: e.repeat,
                    time: e.timeStamp / 1000.0,
                    modifiers: pack_key_modifier(e)
                }
            })
            this.do_wasm_pump();
        };
        ta.addEventListener('keyup', e => this.handlers.on_keyup(e));
        document.body.appendChild(ta);
        ta.focus();
    }


    // internal helper api


    update_text_area_pos(pos) {
        if (this.text_area && pos) {
            //this.text_area.style.left = (Math.round(pos.x) -2) + "px";
            //this.text_area.style.top = (Math.round(pos.y) + 4) + "px"
            this.text_area.style.left = (Math.round(pos.x) - 2) + "px";
            this.text_area.style.top = (Math.round(pos.y) + 4) + "px"
        }
    }

    focus_keyboard_input() {
        if (!this.text_area) return;
        this.text_area.focus();
    }
}

function can_fullscreen() {
    return (document.fullscreenEnabled || document.webkitFullscreenEnabled || document.mozFullscreenEnabled) ? true : false
}

function is_fullscreen() {
    return (document.fullscreenElement || document.webkitFullscreenElement || document.mozFullscreenElement) ? true : false
}

function report_browser_issue(kind, data) {
    try {
        if (typeof window.makepad_report_browser_issue === "function") {
            window.makepad_report_browser_issue(kind, data);
            return;
        }
        const payload = JSON.stringify({
            kind,
            href: location.href,
            user_agent: navigator.userAgent,
            data
        });
        const encoded = encodeURIComponent(payload.slice(0, 8192));
        fetch('/$report_error?data=' + encoded, { cache: 'no-store' });
    } catch (_error) {
    }
}

let web_cursor_map = [
    "none", //Hidden=>0
    "default", //Default=>1,
    "crosshair", //CrossHair=>2,
    "pointer", //Hand=>3,
    "default", //Arrow=>4,
    "move", //Move=>5,
    "text", //Text=>6,
    "wait", //Wait=>7,
    "help", //Help=>8,
    "not-allowed", //NotAllowed=>9,
    "n-resize", // NResize=>10,
    "ne-resize", // NeResize=>11,
    "e-resize", // EResize=>12,
    "se-resize", // SeResize=>13,
    "s-resize", // SResize=>14,
    "sw-resize", // SwResize=>15,
    "w-resize", // WResize=>16,
    "nw-resize", // NwResize=>17,
    "ns-resize", //NsResize=>18,
    "nesw-resize", //NeswResize=>19,
    "ew-resize", //EwResize=>20,
    "nwse-resize", //NwseResize=>21,
    "col-resize", //ColResize=>22,
    "row-resize", //RowResize=>23,
    "grab", //Grab=>24
    "grabbing", //Grabbing=>25    
]

//var firefox_logo_key = false;
function pack_key_modifier(e) {
    return (e.shiftKey ? 1 : 0) | (e.ctrlKey ? 2 : 0) | (e.altKey ? 4 : 0) | (e.metaKey ? 8 : 0)
}
