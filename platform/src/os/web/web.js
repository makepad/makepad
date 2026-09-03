import { WasmBridge } from "../makepad_wasm_bridge/wasm_bridge.js"

const MAKEPAD_CRASH_MAX_REPORTS = 20;
const MAKEPAD_CRASH_POST_BYTES = 64 * 1024;
const MAKEPAD_CRASH_GET_BYTES = 8 * 1024;
const makepad_page_console = {};
for (const level of ["log", "warn", "error"]) {
    const method = typeof console !== "undefined" ? console[level] : undefined;
    makepad_page_console[level] = typeof method === "function" ? method.bind(console) : () => {};
}

function makepad_json_replacer(_key, value) {
    if (typeof value === "bigint") {
        return value.toString();
    }
    if (value instanceof Error) {
        return {
            name: value.name,
            message: value.message,
            stack: value.stack || ""
        };
    }
    if (typeof value === "function") {
        return `[function ${value.name || "anonymous"}]`;
    }
    return value;
}

function makepad_safe_json(value) {
    const seen = new WeakSet();
    try {
        const text = JSON.stringify(value, (key, item) => {
            item = makepad_json_replacer(key, item);
            if (item && typeof item === "object") {
                if (seen.has(item)) {
                    return "[circular]";
                }
                seen.add(item);
            }
            return item;
        });
        return text === undefined ? "null" : text;
    } catch (_error) {
        return JSON.stringify("[unserializable]");
    }
}

function makepad_json_bytes(text) {
    if (typeof TextEncoder !== "undefined") {
        return new TextEncoder().encode(text).byteLength;
    }
    return unescape(encodeURIComponent(text)).length;
}

function makepad_console_text(parts) {
    const values = Array.isArray(parts) ? parts : [parts];
    return values.map(value => {
        if (typeof value === "string") {
            return value;
        }
        if (value instanceof Error) {
            return value.stack || `${value.name}: ${value.message}`;
        }
        if (value && typeof value === "object") {
            return makepad_safe_json(value);
        }
        try {
            return String(value);
        } catch (_error) {
            return "[unprintable]";
        }
    }).join(" ").replace(/\s*\r?\n\s*/g, " ");
}

export function makepad_create_breadcrumb_ring(limit = 30, max_length = 300, started_at = Date.now()) {
    const entries = [];
    return {
        push(level, parts, now = Date.now(), worker_index) {
            try {
                const entry = {
                    ms: Math.max(0, Math.round(now - started_at)),
                    level: String(level),
                    text: makepad_console_text(parts).slice(0, max_length)
                };
                if (worker_index !== undefined && worker_index !== null) {
                    entry.worker = worker_index;
                }
                entries.push(entry);
                if (entries.length > limit) {
                    entries.splice(0, entries.length - limit);
                }
            } catch (_error) {
            }
        },
        snapshot() {
            return entries.map(entry => ({ ...entry }));
        }
    };
}

function makepad_report_message(data) {
    if (!data || typeof data !== "object") {
        return data === undefined ? "" : String(data);
    }
    for (const key of ["message", "reason_message", "text", "error"]) {
        if (data[key] !== undefined && data[key] !== null) {
            return String(data[key]);
        }
    }
    return "";
}

function makepad_report_stack(data) {
    if (!data || typeof data !== "object") {
        return "";
    }
    return String(data.stack || data.reason_stack || data.trap_stack || "");
}

export function makepad_report_key(kind, data) {
    const stack_lines = makepad_report_stack(data)
        .split("\n")
        .map(line => line.trim())
        .filter(Boolean);
    const top_frame = stack_lines.find(line => /^(at\s|.*wasm-function|.*\.wasm(?:\?|:|$))/.test(line))
        || stack_lines[1]
        || stack_lines[0]
        || "";
    return `${kind}\n${makepad_report_message(data)}\n${top_frame}`;
}

export function makepad_create_report_gate(max_reports = MAKEPAD_CRASH_MAX_REPORTS) {
    const seen = new Set();
    let count = 0;
    return {
        accept(kind, data) {
            const key = makepad_report_key(kind, data);
            if (count >= max_reports || seen.has(key)) {
                return false;
            }
            seen.add(key);
            count += 1;
            return true;
        },
        count() {
            return count;
        }
    };
}

export function makepad_truncate_report(report, max_bytes = MAKEPAD_CRASH_GET_BYTES) {
    const fits = value => makepad_json_bytes(value) <= max_bytes;
    let candidate = { ...report };
    let text = makepad_safe_json(candidate);
    if (fits(text)) {
        return text;
    }

    delete candidate.breadcrumbs;
    text = makepad_safe_json(candidate);
    if (fits(text)) {
        return text;
    }

    const original_data = makepad_safe_json(candidate.data);
    candidate.data = { truncated: true, text: "" };
    let low = 0;
    let high = original_data.length;
    while (low < high) {
        const middle = Math.ceil((low + high) / 2);
        candidate.data.text = original_data.slice(0, middle);
        if (fits(makepad_safe_json(candidate))) {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    candidate.data.text = original_data.slice(0, low);
    text = makepad_safe_json(candidate);
    if (fits(text)) {
        return text;
    }

    candidate = {
        v: report.v,
        kind: String(report.kind || "").slice(0, 200),
        app: String(report.app || "").slice(0, 200),
        href: String(report.href || "").slice(0, 512),
        user_agent: String(report.user_agent || "").slice(0, 512),
        time: report.time,
        wasm_memory_bytes: report.wasm_memory_bytes,
        hardware_concurrency: report.hardware_concurrency,
        has_thread_support: report.has_thread_support,
        data: { truncated: true }
    };
    text = makepad_safe_json(candidate);
    if (fits(text)) {
        return text;
    }
    return makepad_safe_json({ v: 1, kind: "report.truncated", data: { truncated: true } });
}

function makepad_is_wasm_trap(kind, data) {
    if (kind !== "window.error" && kind !== "window.unhandledrejection" && kind !== "startup.exception") {
        return false;
    }
    const detail = `${makepad_report_message(data)}\n${makepad_report_stack(data)}`;
    return /WebAssembly(?:\.|\s)|RuntimeError|wasm-function|\.wasm(?:\?|:|\b)|unreachable|memory access out of bounds|table index is out of bounds|null function/i.test(detail);
}

function makepad_create_crash_reporter() {
    const breadcrumbs = makepad_create_breadcrumb_ring();
    const gate = makepad_create_report_gate();
    let wasm = null;
    let page_hiding = false;
    let pending_panic = null;
    let dead = false;
    let dead_error = null;
    let suppressed_followups = 0;

    const app_name = () => {
        try {
            return location.pathname.split("/").filter(Boolean)[0] || "";
        } catch (_error) {
            return "";
        }
    };

    const memory_bytes = () => {
        try {
            const memory = wasm && (wasm._memory || (wasm.exports && wasm.exports.memory));
            return memory && memory.buffer ? memory.buffer.byteLength : null;
        } catch (_error) {
            return null;
        }
    };

    const thread_support = () => {
        try {
            if (wasm && typeof wasm._has_thread_support === "boolean") {
                return wasm._has_thread_support;
            }
            return typeof SharedArrayBuffer !== "undefined"
                && (typeof crossOriginIsolated === "undefined" || crossOriginIsolated === true);
        } catch (_error) {
            return false;
        }
    };

    const build_payload = (kind, data) => ({
        v: 1,
        kind,
        app: app_name(),
        href: typeof location === "undefined" ? "" : String(location.href),
        user_agent: typeof navigator === "undefined" ? "" : String(navigator.userAgent || ""),
        time: Date.now(),
        wasm_memory_bytes: memory_bytes(),
        hardware_concurrency: typeof navigator !== "undefined" && Number.isFinite(navigator.hardwareConcurrency)
            ? navigator.hardwareConcurrency
            : null,
        has_thread_support: thread_support(),
        breadcrumbs: breadcrumbs.snapshot(),
        data
    });

    const fallback_get = async payload => {
        try {
            if (typeof fetch !== "function") {
                return false;
            }
            const text = makepad_truncate_report(payload, MAKEPAD_CRASH_GET_BYTES);
            await fetch('/$report_error?data=' + encodeURIComponent(text), { cache: 'no-store' });
            return true;
        } catch (_error) {
            return false;
        }
    };

    const send = async (kind, data) => {
        try {
            // Keep one slot available for the terminal wasm.dead diagnostic.
            if (kind !== "wasm.dead" && gate.count() >= MAKEPAD_CRASH_MAX_REPORTS - 1) {
                return false;
            }
            if (!gate.accept(kind, data)) {
                return false;
            }
            const payload = build_payload(kind, data);
            const text = makepad_truncate_report(payload, MAKEPAD_CRASH_POST_BYTES);
            if (page_hiding && typeof navigator !== "undefined" && typeof navigator.sendBeacon === "function") {
                try {
                    if (navigator.sendBeacon('/api/crash', new Blob([text], { type: 'application/json' }))) {
                        return true;
                    }
                } catch (_error) {
                }
            }
            if (typeof fetch !== "function") {
                return false;
            }
            const response = await fetch('/api/crash', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: text,
                keepalive: true,
                cache: 'no-store'
            });
            if (response.status === 404 || response.status === 405) {
                return fallback_get(payload);
            }
            return response.ok;
        } catch (_error) {
            return false;
        }
    };

    const take_pending_panic = () => {
        if (!pending_panic || Date.now() - pending_panic.time > 1000) {
            return null;
        }
        const panic = pending_panic;
        pending_panic = null;
        clearTimeout(panic.timer);
        return panic;
    };

    const reporter = {
        report(kind, data) {
            try {
                kind = String(kind || "unknown");
                if (kind === "wasm.panic") {
                    if (pending_panic) {
                        clearTimeout(pending_panic.timer);
                        void send("wasm.panic", pending_panic.data);
                    }
                    const panic = { data, time: Date.now(), timer: null };
                    panic.timer = setTimeout(() => {
                        if (pending_panic === panic) {
                            pending_panic = null;
                            void send("wasm.panic", data);
                        }
                    }, 1000);
                    pending_panic = panic;
                    return Promise.resolve(true);
                }
                if (makepad_is_wasm_trap(kind, data)) {
                    reporter.mark_wasm_dead(data);
                    const panic = take_pending_panic();
                    if (panic) {
                        return send("wasm.panic", {
                            ...panic.data,
                            trap_message: makepad_report_message(data),
                            trap_stack: makepad_report_stack(data)
                        });
                    }
                }
                return send(kind, data);
            } catch (_error) {
                return Promise.resolve(false);
            }
        },
        set_wasm(next_wasm) {
            try {
                wasm = next_wasm;
            } catch (_error) {
            }
        },
        add_breadcrumb(level, parts, worker_index) {
            breadcrumbs.push(level, parts, Date.now(), worker_index);
        },
        mark_wasm_dead(error) {
            try {
                if (dead) {
                    return;
                }
                dead = true;
                dead_error = {
                    message: makepad_report_message(error),
                    stack: makepad_report_stack(error)
                };
                setTimeout(() => {
                    void send("wasm.dead", {
                        suppressed_followups,
                        first_trap: dead_error
                    });
                }, 2000);
            } catch (_error) {
            }
        },
        is_wasm_dead() {
            return dead;
        },
        suppress_followup() {
            if (dead) {
                suppressed_followups += 1;
            }
        },
        set_page_hiding(value) {
            page_hiding = !!value;
            if (page_hiding && pending_panic) {
                const panic = pending_panic;
                pending_panic = null;
                clearTimeout(panic.timer);
                void send("wasm.panic", panic.data);
            }
        }
    };
    return reporter;
}

export const makepad_crash_reporter = makepad_create_crash_reporter();

if (typeof window !== "undefined") {
    window.makepad_crash_reporter = makepad_crash_reporter;
    for (const level of ["log", "warn", "error"]) {
        try {
            const original = console[level];
            if (typeof original !== "function") {
                continue;
            }
            console[level] = function (...parts) {
                makepad_crash_reporter.add_breadcrumb(level, parts);
                return original.apply(console, parts);
            };
        } catch (_error) {
        }
    }
    window.addEventListener("pagehide", () => makepad_crash_reporter.set_page_hiding(true));
    window.addEventListener("pageshow", () => makepad_crash_reporter.set_page_hiding(false));
}

export class WasmWebBrowser extends WasmBridge {
    constructor(wasm, dispatch, canvas) {
        super(wasm, dispatch);
        if (wasm === undefined) {
            return
        }
        makepad_crash_reporter.set_wasm(wasm);
        this.wasm_app = this.wasm_create_app();

        this.create_js_message_bridge(this.wasm_app);

        this.dispatch = dispatch;
        this.canvas = canvas;
        this.handlers = new Proxy({}, {
            set(target, property, value) {
                target[property] = typeof value === "function" ? (...args) => {
                    if (makepad_crash_reporter.is_wasm_dead()) {
                        makepad_crash_reporter.suppress_followup();
                        return;
                    }
                    return value(...args);
                } : value;
                return true;
            }
        });
        this.timers = [];
        this.text_copy_response = "";
        this.web_sockets = [];
        this.network_web_sockets = {};
        this.network_http_requests = new Map();
        this.network_http_hosts = new Map();
        this.storage_db_promise = null;
        this.window_info = {}
        this.xr_capabilities = {
            vr_supported: false,
            ar_supported: false
        };
        this.xr_supported = false;
        this.signal_timeout = null;
        this.workers = new Map();
        this.worker_console_recent = new Map();
        this.thread_stack_arena = [];
        this.thread_stack_size = 2 * 1024 * 1024;
        this.ui_wake_queued = false;
        this.buffer_upload_serial = 0;
        this.loader_removed = false;
        this.loader_seen_animation_frame = false;
        this.loader_quiet_animation_frames = 0;
        this.loader_after_presented_frame_id = 0;
        this.loader_fallback_timer = null;
        this.virtual_file_max_size = 512 * 1024 * 1024;
        this.virtual_file_max_total_size = 512 * 1024 * 1024;
        this.init_detection();
        this.midi_inputs = [];
        this.midi_outputs = [];
        this.audio_context = null;
        this.audio_worklet = null;
        this.audio_callback_started = false;
        this.audio_callback_watchdog = null;

        this.dispatch_first_msg();
    }

    js_monotonic_now() {
        return performance.now() / 1000.0;
    }

    js_wake_ui() {
        if (makepad_crash_reporter.is_wasm_dead()) {
            makepad_crash_reporter.suppress_followup();
            return;
        }
        if (this.ui_wake_queued) {
            return;
        }
        this.ui_wake_queued = true;
        queueMicrotask(() => {
            this.ui_wake_queued = false;
            if (makepad_crash_reporter.is_wasm_dead()) {
                makepad_crash_reporter.suppress_followup();
                return;
            }
            const flags = this.exports.wasm_check_signal();
            if (flags !== 0) {
                this.to_wasm.ToWasmSignal({ flags });
                this.do_wasm_pump();
            }
        });
    }

    js_spawn_thread(request_id, context_ptr, stack_size, name_ptr, name_len) {
        if (!this.wasm._has_thread_support) {
            return 0;
        }
        const name = this.u8_to_string(name_ptr, name_len);
        this.create_thread({ request_id, context_ptr, stack_size, name });
        return 1;
    }

    shutdown_thread_runtime() {
        for (const [request_id, record] of this.workers) {
            record.closed = true;
            record.worker.terminate();
            if (record.started) {
                this.exports.wasm_thread_worker_lost(request_id);
            } else {
                this.exports.wasm_thread_failed_to_start(request_id);
            }
            if (!record.thread_info.wasm_bindgen && record.thread_info.tls_ptr) {
                this.thread_stack_arena.push({
                    ptr: record.thread_info.tls_ptr,
                    words: record.thread_info.alloc_words
                });
            }
        }
        this.workers.clear();
        for (const block of this.thread_stack_arena) {
            this.exports.wasm_thread_dealloc_tls_and_stack(block.ptr, block.words);
        }
        this.thread_stack_arena.length = 0;
    }

    emit_app_lifecycle(state) {
        this.to_wasm.ToWasmAppLifecycle({ state });
    }

    emit_app_inactive() {
        if (!this.lifecycle_is_visible) {
            return;
        }
        this.lifecycle_is_visible = false;
        this.emit_app_lifecycle(2);
        this.emit_app_lifecycle(1);
    }

    emit_app_active() {
        if (this.lifecycle_is_visible) {
            return;
        }
        this.lifecycle_is_visible = true;
        this.lifecycle_shutdown_sent = false;
        this.emit_app_lifecycle(0);
        this.emit_app_lifecycle(3);
    }

    emit_app_shutdown() {
        if (this.lifecycle_shutdown_sent) {
            return;
        }
        this.lifecycle_shutdown_sent = true;
        this.emit_app_lifecycle(4);
    }

    bind_app_lifecycle() {
        this.lifecycle_is_visible = !document.hidden;
        this.lifecycle_shutdown_sent = false;

        document.addEventListener("visibilitychange", () => {
            if (document.hidden) {
                this.emit_app_inactive();
            } else {
                this.emit_app_active();
            }
            this.do_wasm_pump();
        });

        window.addEventListener("pagehide", (event) => {
            this.emit_app_inactive();
            if (!event.persisted) {
                this.emit_app_shutdown();
            }
            this.do_wasm_pump();
            if (!event.persisted) {
                this.shutdown_thread_runtime();
            }
        });

        window.addEventListener("pageshow", (event) => {
            if (event.persisted) {
                this.emit_app_active();
                this.do_wasm_pump();
            }
        });

    }

    emit_location_change() {
        this.to_wasm.ToWasmLocationChange({
            pathname: location.pathname + "",
            search: location.search + "",
            hash: location.hash + "",
        });
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

        const hardware_concurrency = Number.isFinite(navigator.hardwareConcurrency)
            ? Math.max(1, Math.floor(navigator.hardwareConcurrency))
            : 1;
        this.to_wasm.ToWasmInit({
            gpu_info: this.gpu_info,
            cpu_cores: hardware_concurrency,
            wasm_memory_max_pages: this.wasm._memory_max_pages || 16384,
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
        this.bind_file_drop();
        this.bind_keyboard();
        this.bind_screen_resize();
        this.bind_app_lifecycle();
        window.addEventListener("popstate", () => {
            this.emit_location_change();
            this.do_wasm_pump();
        });
        window.addEventListener("hashchange", () => {
            this.emit_location_change();
            this.do_wasm_pump();
        });
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
        if (this.loader_after_presented_frame_id) {
            window.cancelAnimationFrame(this.loader_after_presented_frame_id);
            this.loader_after_presented_frame_id = 0;
        }
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

    cancel_loader_after_presented_frame() {
        if (!this.loader_after_presented_frame_id) {
            return;
        }
        window.cancelAnimationFrame(this.loader_after_presented_frame_id);
        this.loader_after_presented_frame_id = 0;
    }

    schedule_loader_after_presented_frame() {
        if (this.loader_removed || this.loader_after_presented_frame_id) {
            return;
        }
        this.loader_after_presented_frame_id = window.requestAnimationFrame(() => {
            this.loader_after_presented_frame_id = 0;
            if (
                !this.loader_removed &&
                this.loader_seen_animation_frame &&
                this.loader_quiet_animation_frames >= 2
            ) {
                this.remove_canvas_loader();
            }
        });
    }

    update_startup_loader(pump_duration_ms) {
        if (this.loader_removed) {
            return;
        }
        this.schedule_loader_fallback();
        if (!this.in_animation_frame) {
            if (this.loader_seen_animation_frame) {
                this.loader_quiet_animation_frames = 0;
                this.cancel_loader_after_presented_frame();
            }
            return;
        }
        this.loader_seen_animation_frame = true;
        if (pump_duration_ms <= 16) {
            this.loader_quiet_animation_frames += 1;
        }
        else {
            this.loader_quiet_animation_frames = 0;
            this.cancel_loader_after_presented_frame();
            return;
        }
        if (this.loader_quiet_animation_frames >= 2) {
            this.schedule_loader_after_presented_frame();
        }
        else {
            this.cancel_loader_after_presented_frame();
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

    FromWasmBrowserUpdateUrl(args) {
        const next = new URL(args.url || "", window.location.href);
        const nextHref = next.pathname + next.search + next.hash;
        const currentHref = location.pathname + location.search + location.hash;
        if (nextHref === currentHref) {
            return;
        }
        if (args.replace) {
            window.history.replaceState(null, "", nextHref);
        }
        else {
            window.history.pushState(null, "", nextHref);
        }
    }

    FromWasmBrowserHistoryGo(args) {
        if (args.delta === -1) {
            window.history.back();
        }
        else if (args.delta === 1) {
            window.history.forward();
        }
        else {
            window.history.go(args.delta);
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

    FromWasmStartLocationUpdates() {
        if (this.geo_watch_id !== undefined) {
            return; // already watching
        }
        if (!navigator.geolocation) {
            this.to_wasm.ToWasmLocationError({ code: 0, message: "geolocation API unavailable" });
            this.do_wasm_pump();
            return;
        }
        this.geo_watch_id = navigator.geolocation.watchPosition(
            (pos) => {
                let c = pos.coords;
                // Option<f64> encodes as undefined; browser nulls must convert
                this.to_wasm.ToWasmLocationUpdate({
                    lon: c.longitude,
                    lat: c.latitude,
                    accuracy_m: c.accuracy,
                    altitude_m: c.altitude === null ? undefined : c.altitude,
                    speed_mps: c.speed === null ? undefined : c.speed,
                    heading_deg: (c.heading === null || isNaN(c.heading)) ? undefined : c.heading,
                    time: pos.timestamp / 1000.0,
                });
                this.do_wasm_pump();
            },
            (err) => {
                this.to_wasm.ToWasmLocationError({ code: err.code, message: err.message });
                this.do_wasm_pump();
            },
            { enableHighAccuracy: true, maximumAge: 1000 }
        );
    }

    FromWasmStopLocationUpdates() {
        if (this.geo_watch_id !== undefined) {
            navigator.geolocation.clearWatch(this.geo_watch_id);
            this.geo_watch_id = undefined;
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

    storage_database() {
        if (this.storage_db_promise !== null) {
            return this.storage_db_promise;
        }
        this.storage_db_promise = new Promise((resolve, reject) => {
            const request = indexedDB.open("makepad-storage", 1);
            request.onupgradeneeded = () => {
                const db = request.result;
                const store = db.objectStoreNames.contains("values")
                    ? request.transaction.objectStore("values")
                    : db.createObjectStore("values", { keyPath: "id" });
                if (!store.indexNames.contains("namespace")) {
                    store.createIndex("namespace", "namespace", { unique: false });
                }
            };
            request.onsuccess = () => resolve(request.result);
            request.onerror = () => reject(request.error || new Error("could not open IndexedDB"));
            request.onblocked = () => reject(new Error("IndexedDB upgrade was blocked"));
        });
        return this.storage_db_promise;
    }

    storage_id(namespace, key) {
        return namespace + "\u0000" + key;
    }

    storage_error_text(error) {
        if (error && error.message) {
            return error.message;
        }
        return String(error || "unknown IndexedDB error");
    }

    storage_send_result(args, op, result = {}) {
        this.to_wasm.ToWasmStorageResult({
            request_id_lo: args.request_id_lo,
            request_id_hi: args.request_id_hi,
            op,
            found: result.found === true,
            value: result.value || new Uint8Array(0),
            keys: result.keys || [],
            has_next: result.next !== undefined,
            next: result.next || "",
            length_lo: result.length === undefined ? 0 : result.length >>> 0,
            length_hi: result.length === undefined ? 0 : Math.floor(result.length / 0x100000000),
            usage_lo: result.usage === undefined ? 0 : result.usage >>> 0,
            usage_hi: result.usage === undefined ? 0 : Math.floor(result.usage / 0x100000000),
            quota_lo: result.quota === undefined ? 0 : result.quota >>> 0,
            quota_hi: result.quota === undefined ? 0 : Math.floor(result.quota / 0x100000000),
            error_kind: result.error_kind || 0,
            error: result.error || ""
        });
        this.do_wasm_pump();
    }

    storage_request(request) {
        return new Promise((resolve, reject) => {
            request.onsuccess = () => resolve(request.result);
            request.onerror = () => reject(request.error || new Error("IndexedDB request failed"));
        });
    }

    FromWasmStorageGet(args) {
        this.storage_database().then(db => {
            const request = db.transaction("values", "readonly")
                .objectStore("values").get(this.storage_id(args.namespace, args.key));
            return this.storage_request(request);
        }).then(record => {
            this.storage_send_result(args, 0, record === undefined
                ? { found: false }
                : { found: true, value: record.value });
        }).catch(error => {
            this.storage_send_result(args, 0, { error: this.storage_error_text(error) });
        });
    }

    FromWasmStorageSet(args) {
        const value = this.clone_data_u8(args.value);
        this.free_data_u8(args.value);
        this.storage_database().then(db => new Promise((resolve, reject) => {
            const transaction = db.transaction("values", "readwrite");
            transaction.oncomplete = () => resolve();
            transaction.onerror = () => reject(transaction.error || new Error("IndexedDB write failed"));
            transaction.onabort = () => reject(transaction.error || new Error("IndexedDB write aborted"));
            transaction.objectStore("values").put({
                id: this.storage_id(args.namespace, args.key),
                namespace: args.namespace,
                key: args.key,
                value: value.buffer
            });
        })).then(() => {
            this.storage_send_result(args, 1);
        }).catch(error => {
            this.storage_send_result(args, 1, {
                error: this.storage_error_text(error),
                error_kind: error && error.name === "QuotaExceededError" ? 1 : 0
            });
        });
    }

    FromWasmStorageDelete(args) {
        this.storage_database().then(db => new Promise((resolve, reject) => {
            const transaction = db.transaction("values", "readwrite");
            transaction.oncomplete = () => resolve();
            transaction.onerror = () => reject(transaction.error || new Error("IndexedDB delete failed"));
            transaction.onabort = () => reject(transaction.error || new Error("IndexedDB delete aborted"));
            transaction.objectStore("values").delete(this.storage_id(args.namespace, args.key));
        })).then(() => {
            this.storage_send_result(args, 2);
        }).catch(error => {
            this.storage_send_result(args, 2, { error: this.storage_error_text(error) });
        });
    }

    FromWasmStorageList(args) {
        this.storage_database().then(db => new Promise((resolve, reject) => {
            const keys = [];
            const request = db.transaction("values", "readonly")
                .objectStore("values").index("namespace")
                .openCursor(IDBKeyRange.only(args.namespace));
            request.onerror = () => reject(request.error || new Error("IndexedDB cursor failed"));
            request.onsuccess = () => {
                const cursor = request.result;
                if (cursor === null) {
                    resolve(keys);
                    return;
                }
                const key = cursor.value.key;
                if (key.startsWith(args.prefix) && (!args.has_after || key > args.after)) {
                    keys.push(key);
                    if (keys.length > args.limit) {
                        resolve(keys);
                        return;
                    }
                }
                cursor.continue();
            };
        })).then(keys => {
            let next;
            if (keys.length > args.limit) {
                keys.length = args.limit;
                next = keys[keys.length - 1];
            }
            this.storage_send_result(args, 3, { keys, next });
        }).catch(error => {
            this.storage_send_result(args, 3, { error: this.storage_error_text(error) });
        });
    }

    FromWasmStorageGetRange(args) {
        this.storage_database().then(db => {
            const request = db.transaction("values", "readonly")
                .objectStore("values").get(this.storage_id(args.namespace, args.key));
            return this.storage_request(request);
        }).then(record => {
            if (record === undefined) {
                this.storage_send_result(args, 4, { found: false });
                return;
            }
            const value = new Uint8Array(record.value);
            const offset = args.offset_hi > 0x1fffff
                ? Number.MAX_SAFE_INTEGER
                : args.offset_lo + args.offset_hi * 0x100000000;
            const end = Math.min(value.length, offset + args.len);
            const range = offset >= value.length ? new Uint8Array(0) : value.slice(offset, end);
            this.storage_send_result(args, 4, { found: true, value: range });
        }).catch(error => {
            this.storage_send_result(args, 4, { error: this.storage_error_text(error) });
        });
    }

    FromWasmStorageStat(args) {
        this.storage_database().then(db => {
            const request = db.transaction("values", "readonly")
                .objectStore("values").get(this.storage_id(args.namespace, args.key));
            return this.storage_request(request);
        }).then(record => {
            this.storage_send_result(args, 5, record === undefined
                ? { found: false }
                : { found: true, length: record.value.byteLength });
        }).catch(error => {
            this.storage_send_result(args, 5, { error: this.storage_error_text(error) });
        });
    }

    FromWasmStorageEstimate(args) {
        const estimate = navigator.storage && navigator.storage.estimate;
        if (!estimate) {
            this.storage_send_result(args, 6, { error: "storage estimate is unavailable" });
            return;
        }
        navigator.storage.estimate().then(result => {
            this.storage_send_result(args, 6, {
                usage: Math.max(0, Math.floor(result.usage || 0)),
                quota: Math.max(0, Math.floor(result.quota || 0))
            });
        }).catch(error => {
            this.storage_send_result(args, 6, { error: this.storage_error_text(error) });
        });
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
        if (this.audio_callback_watchdog !== null) {
            clearTimeout(this.audio_callback_watchdog);
            this.audio_callback_watchdog = null;
        }
        if (this.audio_worklet) {
            this.audio_worklet.disconnect();
            this.audio_worklet = null;
        }
        const audio_context = this.audio_context;
        this.audio_context = null;
        audio_context.close().catch(error => {
            console.error(`web audio: close failed: ${error}`);
        });
    }

    watch_audio_callback(audio_context) {
        if (!this.audio_worklet
            || this.audio_callback_started
            || this.audio_callback_watchdog !== null) {
            return;
        }
        this.audio_callback_watchdog = setTimeout(() => {
            this.audio_callback_watchdog = null;
            if (this.audio_context === audio_context && !this.audio_callback_started) {
                console.error(
                    `web audio: callback never called state=${audio_context.state} sample_rate=${audio_context.sampleRate} buffer=pending`,
                );
            }
        }, 3000);
    }

    resume_audio_from_gesture() {
        this.had_user_gesture = true;
        if (!this.audio_context && this.audio_start_args) {
            const args = this.audio_start_args;
            this.audio_start_args = null;
            this.start_audio_output(args, 1);
            return;
        }
        const audio_context = this.audio_context;
        if (!audio_context) {
            return;
        }
        if (audio_context.state === "running") {
            this.watch_audio_callback(audio_context);
            return;
        }
        if (audio_context.state !== "suspended" || audio_context._makepad_resume_pending) {
            return;
        }
        audio_context._makepad_resume_pending = true;
        audio_context.resume().then(() => {
            audio_context._makepad_resume_pending = false;
            if (this.audio_context !== audio_context) {
                return;
            }
            if (audio_context.state === "running") {
                this.watch_audio_callback(audio_context);
            } else {
                console.error(
                    `web audio: context suspended after canvas gesture state=${audio_context.state} sample_rate=${audio_context.sampleRate} buffer=pending`,
                );
            }
        }).catch(error => {
            audio_context._makepad_resume_pending = false;
            console.error(
                `web audio: resume failed state=${audio_context.state} sample_rate=${audio_context.sampleRate} buffer=pending: ${error}`,
            );
        });
    }

    FromWasmStartAudioOutput(args) {
        if (this.audio_context) {
            return
        }
        // The web's rule: an output is created inside a user gesture. The wasm asks at
        // start-up; the first click or key press creates the context and the worklet.
        if (!this.had_user_gesture) {
            this.audio_start_args = args;
            return;
        }
        this.start_audio_output(args, 1);
    }

    start_audio_output(args, attempt) {
        if (this.audio_context) {
            return
        }
        let audio_context;
        try {
            audio_context = new AudioContext({
                latencyHint: "interactive"
            });
        } catch (error) {
            console.error(`web audio: context creation failed: ${error}`);
            return;
        }
        this.audio_context = audio_context;
        this.audio_callback_started = false;

        const start_worklet = async () => {
            if (this.wasm._secondary_ready) {
                await this.wasm._secondary_ready;
            }
            if (!this.wasm._has_thread_support) {
                throw new Error("wasm threading support is unavailable");
            }
            const thread_info = this.alloc_thread_stack(args.context_ptr);
            if (!thread_info) {
                throw new Error("thread stack allocation prerequisites are unavailable");
            }

            // A stalled module load (seen: it never settles until a second context exists)
            // is not waited on forever — the deadline fails this attempt, and the retry below
            // starts over on a fresh context.
            await Promise.race([
                audio_context.audioWorklet.addModule("./makepad_platform/audio_worklet.js", { credentials: 'omit' }),
                new Promise((_, reject) => setTimeout(() => reject(new Error("worklet module load stalled")), 4000)),
            ]);

            const audio_worklet = new AudioWorkletNode(audio_context, 'audio-worklet', {
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

                    case "wake_ui":
                        // the audio thread raised the UI signal (meters, transport state): pump like a wake
                        this.do_wasm_pump();
                        break;

                    case "audio_callback_started":
                        this.audio_callback_started = true;
                        if (this.audio_callback_watchdog !== null) {
                            clearTimeout(this.audio_callback_watchdog);
                            this.audio_callback_watchdog = null;
                        }
                        console.log(
                            `web audio: callback running state=${audio_context.state} sample_rate=${data.sample_rate} buffer=${data.frames}x${data.channels}`,
                        );
                        break;
                }
            };
            audio_worklet.onprocessorerror = (err) => {
                console.error(`web audio: processor failed: ${err}`);
            }
            audio_worklet.connect(audio_context.destination);

            return audio_worklet;
        };

        start_worklet().then(audio_worklet => {
            if (this.audio_context !== audio_context) {
                audio_worklet.disconnect();
                return;
            }
            this.audio_worklet = audio_worklet;
            if (audio_context.state === "running") {
                this.watch_audio_callback(audio_context);
            }
        }).catch(error => {
            console.error(`web audio: start failed (attempt ${attempt}): ${error}`);
            if (this.audio_context !== audio_context) {
                return;
            }
            this.audio_context = null;
            audio_context.close().catch(() => {});
            if (attempt < 3) {
                this.start_audio_output(args, attempt + 1);
            }
        });
    }

    FromWasmQueryAudioDevices(args) {
        const publish_devices = (devices_enum) => {
            let devices = []
            for (let device of devices_enum) {
                if (device.kind == "audioinput") {
                    devices.push({
                        web_device_id: "" + device.deviceId,
                        label: "" + device.label,
                        is_output: false
                    });
                }
            }
            // AudioContext.destination is the browser-selected output. Until
            // this backend supports setSinkId, expose that one honest route
            // instead of device choices FromWasmStartAudioOutput cannot use.
            const output = devices_enum.find(device =>
                device.kind == "audiooutput" && device.deviceId == "default"
            ) || devices_enum.find(device => device.kind == "audiooutput");
            devices.push({
                web_device_id: output ? "" + output.deviceId : "default",
                label: output && output.label ? "" + output.label : "Browser audio",
                is_output: true
            });
            this.to_wasm.ToWasmAudioDeviceList({ devices });
            this.do_wasm_pump();
        };
        const query = navigator.mediaDevices?.enumerateDevices();
        if (!query) {
            console.warn("web audio: device enumeration unavailable; using browser default");
            publish_devices([]);
            return;
        }
        query.then(publish_devices).catch(error => {
            console.warn(`web audio: device enumeration failed; using browser default: ${error}`);
            publish_devices([]);
        });
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
            }, () => {});
        }
    }

    FromWasmStartPresentingXR() {

    }

    alloc_thread_stack(request_id, context_ptr, requested_stack_size) {
        if (!this.wasm._has_thread_support) {
            console.warn("alloc_thread_stack unavailable: wasm threading support is disabled");
            return null;
        }
        if (this.exports.__stack_pointer === undefined) {
            console.warn("alloc_thread_stack unavailable: missing __stack_pointer export");
            return null;
        }
        var ret = {
            request_id,
            module: this.wasm._module,
            secondary_module: this.wasm._secondary_module,
            memory: this.wasm._memory,
            context_ptr
        };
        if (typeof this.exports.__wbindgen_start !== 'undefined') {
            if (requested_stack_size && requested_stack_size !== this.thread_stack_size) {
                console.warn("custom worker stack size is unavailable for wasm-bindgen workers");
                return null;
            }
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
            const raw_tls_size = this.exports.__tls_size.value;
            const tls_size = (raw_tls_size + 7) & ~7;
            // Manual workers use a real 2 MiB default. A validated Rust
            // ThreadOptions::stack_size can override it per worker.
            const stack_size = requested_stack_size || this.thread_stack_size;
            if (((tls_size + stack_size) & 7) !== 0) {
                console.warn("alloc_thread_stack unavailable: stack size is not 8-byte aligned");
                return null;
            }
            const alloc_words = (tls_size + stack_size) >> 3;
            const arena_index = this.thread_stack_arena.findIndex(block => block.words === alloc_words);
            if (arena_index >= 0) {
                const block = this.thread_stack_arena.splice(arena_index, 1)[0];
                ret.tls_ptr = block.ptr;
            } else {
                ret.tls_ptr = this.exports.wasm_thread_alloc_tls_and_stack(alloc_words);
                this.update_array_buffer_refs();
            }
            ret.alloc_words = alloc_words;
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
    create_thread(args) {
        let allocated_thread_info = null;
        (async () => {
            if (this.wasm._secondary_ready) {
                await this.wasm._secondary_ready;
            }
            if (!this.wasm._has_thread_support) {
                throw new Error("wasm file was not compiled with threading support");
            }
            let thread_info = this.alloc_thread_stack(args.request_id, args.context_ptr, args.stack_size);
            if (!thread_info) {
                throw new Error("thread stack allocation prerequisites are missing");
            }
            allocated_thread_info = thread_info;
            let worker = new Worker(
                './makepad_platform/web_worker.js',
                { type: 'module', name: args.name || `makepad-worker-${args.request_id}` }
            );
            const record = { worker, thread_info, started: false, closed: false };
            this.workers.set(args.request_id, record);
            const release = () => {
                if (record.closed) {
                    return;
                }
                record.closed = true;
                worker.terminate();
                this.workers.delete(args.request_id);
                if (!thread_info.wasm_bindgen && thread_info.tls_ptr) {
                    this.thread_stack_arena.push({ ptr: thread_info.tls_ptr, words: thread_info.alloc_words });
                }
            };
            worker.onmessage = event => {
                const message = event.data || {};
                const message_kind = message.kind || message.type;
                if (message_kind === 'breadcrumb') {
                    const level = ["log", "warn", "error"].includes(message.level)
                        ? message.level
                        : "log";
                    const text = message.text || "";
                    makepad_crash_reporter.add_breadcrumb(`worker.${level}`, [text], args.request_id);
                    const key = `${level}\n${text}`;
                    const now = performance.now();
                    const previous = this.worker_console_recent.get(key);
                    this.worker_console_recent.set(key, now);
                    if (previous === undefined || now - previous > 100) {
                        makepad_page_console[level](`[worker ${args.request_id}] ${text}`);
                    }
                    if (this.worker_console_recent.size > 100) {
                        for (const [old_key, seen_at] of this.worker_console_recent) {
                            if (now - seen_at > 100) {
                                this.worker_console_recent.delete(old_key);
                            }
                        }
                    }
                } else if (message_kind === 'panic') {
                    record.last_panic = { text: String(message.text || ""), time: Date.now() };
                } else if (message_kind === 'spawn_request') {
                    this.create_thread(message);
                } else if (message_kind === 'wake_ui') {
                    this.js_wake_ui();
                } else if (message_kind === 'started') {
                    record.started = true;
                    this.exports.wasm_thread_started(args.request_id);
                } else if (message_kind === 'finished') {
                    this.exports.wasm_thread_finished(args.request_id);
                    release();
                } else if (message_kind === 'trapped') {
                    report_browser_issue("worker.error", {
                        worker_index: args.request_id,
                        message: message.message || message.error || "worker trapped",
                        filename: message.filename || "",
                        lineno: message.lineno || 0,
                        stack: message.stack || "",
                        panic: record.last_panic && Date.now() - record.last_panic.time <= 1000
                            ? record.last_panic.text
                            : ""
                    });
                    this.exports.wasm_thread_worker_lost(args.request_id);
                    release();
                } else if (message_kind === 'failed_to_start') {
                    report_browser_issue("worker.error", {
                        worker_index: args.request_id,
                        message: message.message || message.error || "worker failed to start",
                        filename: message.filename || "",
                        lineno: message.lineno || 0,
                        stack: message.stack || ""
                    });
                    this.exports.wasm_thread_failed_to_start(args.request_id);
                    release();
                }
            };
            const on_worker_error = error => {
                if (record.closed) {
                    return;
                }
                console.error(error);
                report_browser_issue("worker.error", {
                    worker_index: args.request_id,
                    message: error && error.message ? String(error.message) : String(error),
                    filename: error && error.filename ? String(error.filename) : "",
                    lineno: error && error.lineno ? error.lineno : 0,
                    colno: error && error.colno ? error.colno : 0,
                    stack: error && error.error && error.error.stack ? String(error.error.stack) : "",
                    panic: record.last_panic && Date.now() - record.last_panic.time <= 1000
                        ? record.last_panic.text
                        : ""
                });
                if (record.started) {
                    this.exports.wasm_thread_worker_lost(args.request_id);
                } else {
                    this.exports.wasm_thread_failed_to_start(args.request_id);
                }
                release();
            };
            worker.onerror = on_worker_error;
            worker.addEventListener("error", on_worker_error);
            worker.addEventListener("messageerror", error => {
                if (record.closed) {
                    return;
                }
                report_browser_issue("worker.error", {
                    worker_index: args.request_id,
                    message: error && error.message ? String(error.message) : "worker message could not be decoded",
                    filename: "",
                    lineno: 0,
                    stack: ""
                });
                if (record.started) {
                    this.exports.wasm_thread_worker_lost(args.request_id);
                } else {
                    this.exports.wasm_thread_failed_to_start(args.request_id);
                }
                release();
            });
            worker.postMessage(thread_info);
        })().catch(err => {
            console.error(err);
            report_browser_issue("worker.error", {
                worker_index: args.request_id,
                message: err && err.message ? String(err.message) : String(err),
                filename: err && err.fileName ? String(err.fileName) : "",
                lineno: err && err.lineNumber ? err.lineNumber : 0,
                stack: err && err.stack ? String(err.stack) : ""
            });
            const record = this.workers.get(args.request_id);
            if (record) {
                record.closed = true;
                record.worker.terminate();
                this.workers.delete(args.request_id);
            }
            if (allocated_thread_info && !allocated_thread_info.wasm_bindgen) {
                this.thread_stack_arena.push({
                    ptr: allocated_thread_info.tls_ptr,
                    words: allocated_thread_info.alloc_words
                });
            }
            this.exports.wasm_thread_failed_to_start(args.request_id);
        });
    }

    start_signal_poll() {
        this.poll_timer = window.setInterval(e => {
            if (makepad_crash_reporter.is_wasm_dead()) {
                return;
            }
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
        body_len,
        max_body_lo,
        max_body_hi
    ) {
        let url = this.u8_to_string(url_ptr, url_len);
        let method = this.u8_to_string(method_ptr, method_len);
        let headers_raw = this.u8_to_string(headers_ptr, headers_len);
        let body = body_len > 0 ? this.u8_to_array(body_ptr, body_len) : undefined;
        let request_key = this.id_to_key(request_id_lo, request_id_hi);
        let max_body = max_body_lo + max_body_hi * 4294967296;
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

        let parsed_url;
        try {
            parsed_url = new URL(url, window.location.href);
        }
        catch (_error) {
            parsed_url = new URL(window.location.href);
        }
        const telemetry = new URLSearchParams(parsed_url.hash.slice(1));
        const is_archive = telemetry.get("makepad-http") === "archive";
        const is_archive_range = is_archive && telemetry.get("range") === "1";
        const tile_keys = telemetry.get("tiles") || "-";
        const priority = Number(telemetry.get("priority") || Number.MAX_SAFE_INTEGER);
        const range_bytes = Number(telemetry.get("bytes") || 0);
        parsed_url.hash = "";
        const fetch_url = parsed_url.href;
        const host_key = parsed_url.origin;
        const entry = {
            request_key,
            request_id_lo,
            request_id_hi,
            metadata_id_lo,
            metadata_id_hi,
            fetch_url,
            method,
            headers,
            body,
            max_body,
            host_key,
            is_archive,
            is_archive_range,
            tile_keys,
            priority,
            range_bytes,
            state: "queued",
            controller: null,
            stall_timer: null,
            started_at: 0,
            response_bytes: 0,
        };
        this.network_http_requests.set(request_key, entry);
        if (!is_archive) {
            this.network_http_dispatch(entry);
            return;
        }
        let host = this.network_http_hosts.get(host_key);
        if (!host) {
            host = { active: 0, pending: 0, queue: [], round: null };
            this.network_http_hosts.set(host_key, host);
        }
        host.pending += 1;
        host.queue.push(entry);
        host.queue.sort((left, right) =>
            left.priority - right.priority
            || right.range_bytes - left.range_bytes
            || left.fetch_url.localeCompare(right.fetch_url)
        );
        this.network_http_pump(host_key);
    }

    network_http_pump(host_key) {
        const host = this.network_http_hosts.get(host_key);
        if (!host) {
            return;
        }
        while (host.active < 5 && host.queue.length > 0) {
            const entry = host.queue.shift();
            if (this.network_http_requests.get(entry.request_key) !== entry) {
                continue;
            }
            host.active += 1;
            this.network_http_dispatch(entry);
        }
    }

    network_http_dispatch(entry) {
        entry.state = "active";
        entry.controller = new AbortController();
        entry.started_at = performance.now();
        if (entry.is_archive_range) {
            let host = this.network_http_hosts.get(entry.host_key);
            if (!host.round) {
                host.round = {
                    started_at: entry.started_at,
                    ranges: 0,
                    bytes: 0,
                    tiles: new Set(),
                };
            }
            host.round.ranges += 1;
            for (const key of entry.tile_keys.split(",")) {
                if (key && key !== "-") {
                    host.round.tiles.add(key);
                }
            }
        }
        const arm_stall_timer = () => {
            if (entry.stall_timer !== null) {
                window.clearTimeout(entry.stall_timer);
            }
            entry.stall_timer = window.setTimeout(() => {
                entry.controller.abort("HTTP request stalled for 20 seconds after dispatch");
            }, 20000);
        };
        arm_stall_timer();
        this.exports.wasm_network_http_progress(
            entry.request_id_lo,
            entry.request_id_hi,
            0,
            0
        );
        fetch(entry.fetch_url, {
            method: entry.method,
            headers: entry.headers,
            body: entry.body,
            signal: entry.controller.signal,
            redirect: "manual",
        }).then(async response => {
            arm_stall_timer();
            let response_headers = "";
            response.headers.forEach((value, key) => {
                response_headers += `${key}: ${value}\r\n`;
            });
            const declared = response.headers.get("content-length");
            if (entry.method !== "HEAD" && declared !== null && Number(declared) > entry.max_body) {
                entry.controller.abort();
                throw "response body exceeds configured limit";
            }
            let chunks = [];
            let response_body_len = 0;
            if (response.body !== null) {
                const reader = response.body.getReader();
                for (;;) {
                    const item = await reader.read();
                    if (item.done) {
                        break;
                    }
                    arm_stall_timer();
                    response_body_len += item.value.byteLength;
                    if (response_body_len > entry.max_body) {
                        entry.controller.abort();
                        throw "response body exceeds configured limit";
                    }
                    chunks.push(item.value);
                }
            }
            let response_body = new Uint8Array(response_body_len);
            let body_at = 0;
            for (const chunk of chunks) {
                response_body.set(chunk, body_at);
                body_at += chunk.byteLength;
            }
            let headers_u8 = this.string_to_u8(response_headers);
            let body_u8 = this.array_to_u8(response_body);
            entry.response_bytes = response_body.length;
            if (response.status >= 400) {
                console.error("[makepad][http][fail]", response.status, entry.fetch_url);
            }
            this.exports.wasm_network_http_response(
                entry.request_id_lo,
                entry.request_id_hi,
                entry.metadata_id_lo,
                entry.metadata_id_hi,
                response.status,
                headers_u8.ptr,
                headers_u8.len,
                body_u8.ptr,
                body_u8.len
            );
        }).catch(error => {
            console.error(
                "[makepad][http][err]",
                entry.method,
                entry.fetch_url,
                "" + error,
                `tiles=${entry.tile_keys}`
            );
            let message_u8 = this.string_to_u8("" + error);
            this.exports.wasm_network_http_error(
                entry.request_id_lo,
                entry.request_id_hi,
                entry.metadata_id_lo,
                entry.metadata_id_hi,
                message_u8.ptr,
                message_u8.len
            );
        }).finally(() => {
            this.network_http_finish(entry);
        });
    }

    network_http_finish(entry) {
        if (entry.stall_timer !== null) {
            window.clearTimeout(entry.stall_timer);
            entry.stall_timer = null;
        }
        if (this.network_http_requests.get(entry.request_key) === entry) {
            this.network_http_requests.delete(entry.request_key);
        }
        entry.state = "done";
        if (!entry.is_archive) {
            return;
        }
        const host = this.network_http_hosts.get(entry.host_key);
        if (!host) {
            return;
        }
        host.active = Math.max(0, host.active - 1);
        host.pending = Math.max(0, host.pending - 1);
        if (host.round && entry.is_archive_range) {
            host.round.bytes += entry.response_bytes;
        }
        this.network_http_pump(entry.host_key);
        if (host.pending === 0) {
            if (host.round) {
                host.round = null;
            }
            if (host.active === 0 && host.queue.length === 0) {
                this.network_http_hosts.delete(entry.host_key);
            }
        }
    }

    js_network_http_cancel(request_id_lo, request_id_hi) {
        let request_key = this.id_to_key(request_id_lo, request_id_hi);
        let entry = this.network_http_requests.get(request_key);
        if (!entry) {
            return;
        }
        if (entry.is_archive && entry.state === "queued") {
            const host = this.network_http_hosts.get(entry.host_key);
            if (host) {
                host.queue = host.queue.filter(item => item !== entry);
                host.pending = Math.max(0, host.pending - 1);
                if (host.pending === 0 && host.round) {
                    host.round = null;
                }
            }
            this.network_http_requests.delete(request_key);
            entry.state = "cancelled";
            const message_u8 = this.string_to_u8("HTTP request cancelled before dispatch");
            this.exports.wasm_network_http_error(
                entry.request_id_lo,
                entry.request_id_hi,
                entry.metadata_id_lo,
                entry.metadata_id_hi,
                message_u8.ptr,
                message_u8.len
            );
            if (host && host.pending === 0 && host.active === 0) {
                this.network_http_hosts.delete(entry.host_key);
            }
            return;
        }
        if (entry.is_archive && entry.state === "active") {
            return;
        }
        entry.controller.abort();
        this.network_http_requests.delete(request_key);
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

    FromWasmSetVirtualFileLimits(args) {
        this.virtual_file_max_size = args.max_file_size;
        this.virtual_file_max_total_size = args.max_total_size;
    }

    read_virtual_files(file_list, max_file_size, max_total_size) {
        const files = Array.from(file_list);
        let total = 0;
        for (const file of files) {
            if (file.size > max_file_size) {
                return Promise.reject(new Error(
                    `file '${file.name}' is ${file.size} bytes, exceeding the per-file limit of ${max_file_size} bytes`
                ));
            }
            total += file.size;
            if (!Number.isSafeInteger(total) || total > max_total_size) {
                return Promise.reject(new Error(
                    `selected files total ${total} bytes, exceeding the per-drop limit of ${max_total_size} bytes`
                ));
            }
        }
        return Promise.all(files.map(async file => {
            const bytes = await file.arrayBuffer();
            if (bytes.byteLength > max_file_size) {
                throw new Error(
                    `file '${file.name}' grew to ${bytes.byteLength} bytes, exceeding the per-file limit of ${max_file_size} bytes`
                );
            }
            return {
                name: file.name + "",
                mime: file.type + "",
                bytes,
            };
        })).then(loaded => {
            const loaded_total = loaded.reduce((sum, file) => sum + file.bytes.byteLength, 0);
            if (!Number.isSafeInteger(loaded_total) || loaded_total > max_total_size) {
                throw new Error(
                    `loaded files total ${loaded_total} bytes, exceeding the per-drop limit of ${max_total_size} bytes`
                );
            }
            return loaded;
        });
    }

    FromWasmSelectFileDialog(args) {
        const input = document.createElement('input');
        input.type = 'file';
        input.style.display = 'none';
        input.multiple = args.multiple;
        if (args.accept) {
            input.accept = args.accept;
        }
        document.body.appendChild(input);

        let settled = false;
        let selection_started = false;
        let picker_blurred_window = false;
        const cleanup = () => {
            window.removeEventListener('blur', on_blur);
            window.removeEventListener('focus', on_focus);
            if (input.parentNode) {
                input.parentNode.removeChild(input);
            }
        };
        const finish = (cancelled, files, error) => {
            if (settled) {
                return;
            }
            settled = true;
            cleanup();
            this.to_wasm.ToWasmFileDialogResult({
                id_lo: args.id_lo,
                id_hi: args.id_hi,
                cancelled,
                error,
                files,
            });
            this.do_wasm_pump();
        };
        const cancel = () => finish(true, [], "");
        const on_blur = () => {
            picker_blurred_window = true;
        };
        const on_focus = () => {
            if (picker_blurred_window) {
                window.setTimeout(() => {
                    if (!selection_started) {
                        cancel();
                    }
                }, 0);
            }
        };

        input.addEventListener('change', () => {
            selection_started = true;
            if (!input.files || input.files.length === 0) {
                cancel();
                return;
            }
            this.read_virtual_files(input.files, args.max_file_size, args.max_total_size)
                .then(files => finish(false, files, ""))
                .catch(error => finish(true, [], "" + error));
        });
        // Modern browsers report picker dismissal directly. The blur/focus
        // pair below is the fallback for older engines; engines that expose
        // neither signal cannot report cancellation until another result.
        input.addEventListener('cancel', cancel);
        window.addEventListener('blur', on_blur);
        window.addEventListener('focus', on_focus);

        // This method is dispatched synchronously by do_wasm_pump(), so the
        // click stays in the button/mouse event task whenever the app queued
        // the dialog from that handler. Browsers reject later, unprompted calls.
        try {
            input.click();
        }
        catch (error) {
            finish(true, [], "browser rejected file picker: " + error);
        }
    }

    async FromWasmCheckPermission(args) {
        try {
            if (args.permission === 'microphone' || args.permission === 'camera' || args.permission === 'geolocation') {
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
                } else if (args.permission === 'geolocation') {
                    // No Permissions API — cannot check without prompting
                    this.to_wasm.ToWasmPermissionResult({
                        permission: args.permission,
                        request_id: args.request_id,
                        status: 0 // NotDetermined
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
            } else if (args.permission === 'geolocation' && navigator.geolocation) {
                // One-shot position read triggers the browser prompt;
                // callbacks pump for themselves.
                navigator.geolocation.getCurrentPosition(
                    (_pos) => {
                        this.to_wasm.ToWasmPermissionResult({
                            permission: args.permission,
                            request_id: args.request_id,
                            status: 1 // Granted
                        });
                        this.do_wasm_pump();
                    },
                    (err) => {
                        this.to_wasm.ToWasmPermissionResult({
                            permission: args.permission,
                            request_id: args.request_id,
                            status: err.code === 1 ? 3 : 2 // denied : retryable
                        });
                        this.do_wasm_pump();
                    },
                    { timeout: 30000 }
                );
                return;
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
        if (makepad_crash_reporter.is_wasm_dead()) {
            makepad_crash_reporter.suppress_followup();
            return;
        }
        let started = performance.now();
        try {
            this.buffer_upload_serial += 1;
            let to_wasm = this.to_wasm;
            this.to_wasm = this.new_to_wasm();
            let from_wasm = this.wasm_process_msg(to_wasm);
            from_wasm.dispatch_on_app();
            from_wasm.free();
            this.update_startup_loader(performance.now() - started);
        } catch (error) {
            makepad_crash_reporter.mark_wasm_dead(error);
            void makepad_crash_reporter.report("window.error", {
                message: error && error.message ? String(error.message) : String(error),
                stack: error && error.stack ? String(error.stack) : ""
            });
            throw error;
        }
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
            this.resume_audio_from_gesture();
            e.preventDefault();
            this.focus_keyboard_input();
            //if (current_mouse_down === null || current_mouse_down === e.button){
            //    current_mouse_down = e.button;
            this.to_wasm.ToWasmMouseDown({ mouse: mouse_to_wasm_wmouse(e) });
            this.do_wasm_pump();
            // The gesture can synchronously cause the app to open its first
            // output. Resume that newly-created context before returning to
            // the browser and losing user activation.
            this.resume_audio_from_gesture();
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
            this.resume_audio_from_gesture();
            e.preventDefault()
            this.to_wasm.ToWasmTouchUpdate({
                time: e.timeStamp / 1000.0,
                modifiers: pack_key_modifier(e),
                touches: touches_to_wasm_wtouches(e, 1)
            });
            this.do_wasm_pump();
            this.resume_audio_from_gesture();
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

    bind_file_drop() {
        const canvas = this.canvas;
        const file_count = event => {
            if (event.dataTransfer.files && event.dataTransfer.files.length) {
                return event.dataTransfer.files.length;
            }
            return Array.from(event.dataTransfer.items || [])
                .filter(item => item.kind === 'file').length;
        };
        const position = event => {
            const rect = canvas.getBoundingClientRect();
            return {
                x: event.clientX - rect.left,
                y: event.clientY - rect.top,
            };
        };
        const emit_drag = (event, left) => {
            const pos = position(event);
            this.to_wasm.ToWasmFileDrag({
                x: pos.x,
                y: pos.y,
                modifiers: pack_key_modifier(event),
                file_count: file_count(event),
                left,
            });
            this.do_wasm_pump();
        };

        canvas.addEventListener('dragenter', event => {
            event.preventDefault();
            emit_drag(event, false);
        });
        canvas.addEventListener('dragover', event => {
            event.preventDefault();
            if (event.dataTransfer) {
                event.dataTransfer.dropEffect = 'copy';
            }
            emit_drag(event, false);
        });
        canvas.addEventListener('dragleave', event => {
            event.preventDefault();
            emit_drag(event, true);
        });
        canvas.addEventListener('drop', event => {
            event.preventDefault();
            const pos = position(event);
            const modifiers = pack_key_modifier(event);
            this.read_virtual_files(
                event.dataTransfer.files,
                this.virtual_file_max_size,
                this.virtual_file_max_total_size,
            ).then(files => {
                this.to_wasm.ToWasmFileDrop({
                    x: pos.x,
                    y: pos.y,
                    modifiers,
                    files,
                });
                this.do_wasm_pump();
            }).catch(error => {
                this.to_wasm.ToWasmFileDropError({error: "" + error});
                this.do_wasm_pump();
            });
        });
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
            this.resume_audio_from_gesture();
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
            this.resume_audio_from_gesture();
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
        makepad_crash_reporter.report(kind, data);
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
