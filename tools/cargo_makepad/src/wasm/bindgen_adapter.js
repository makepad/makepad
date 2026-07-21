import init from "./bindgen.js";

export async function init_makepad_bindgen(wasm_url, init_env) {
    const env = {};
    const set_wasm = init_env(env);
    const module = await WebAssembly.compileStreaming(fetch(wasm_url));
    const exports = await init({ module_or_path: module }, env);
    const wasm = { exports };
    const memory = env.memory ?? exports.memory;

    wasm._has_thread_support = typeof SharedArrayBuffer !== "undefined"
        && memory.buffer instanceof SharedArrayBuffer;
    wasm._memory = memory;
    wasm._module = module;
    set_wasm(wasm);

    return wasm;
}
