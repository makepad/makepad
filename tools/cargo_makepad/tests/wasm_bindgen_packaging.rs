use cargo_makepad::{generate_html, WasmConfig};

fn config(bindgen: bool) -> WasmConfig {
    WasmConfig {
        lan: false,
        port: None,
        small_fonts: false,
        brotli: false,
        bindgen,
        threads: true,
        optimize_size: false,
        wasm_opt: false,
        hot_reload: false,
    }
}

#[test]
fn bindgen_html_loads_makepad_bridge_and_bindgen_adapter() {
    let html = generate_html("makepad_example_bindgen_web", &config(true));

    assert!(html.contains("./makepad_wasm_bridge/wasm_bridge.js"));
    assert!(html.contains("./bindgen_adapter.js"));
    assert!(html.contains("const wasm = await init_makepad_bindgen('./makepad_example_bindgen_web.wasm', init_env);"));
    assert!(!html.contains("const init = (await import('./bindgen.js')).default;"));
}

#[test]
fn non_bindgen_html_keeps_direct_makepad_bootstrap() {
    let html = generate_html("makepad_example_counter", &config(false));

    assert!(html.contains("WasmWebGL.fetch_and_instantiate_wasm"));
    assert!(!html.contains("./bindgen_adapter.js"));
    assert!(!html.contains("init_makepad_bindgen"));
}

#[test]
fn bindgen_adapter_owns_makepad_env_integration() {
    let adapter = include_str!("../src/wasm/bindgen_adapter.js");

    assert!(adapter.contains("import init from \"./bindgen.js\""));
    assert!(adapter.contains("init_env(env)"));
    assert!(adapter.contains("const exports = await init({ module_or_path: module }, env);"));
    assert!(adapter.contains("const wasm = { exports };"));
    assert!(adapter.contains("const memory = env.memory ?? exports.memory;"));
    assert!(adapter.contains("wasm._memory = memory;"));
    assert!(adapter.contains("set_wasm(wasm)"));
    assert!(adapter.contains("wasm._has_thread_support"));
}

#[test]
fn bindgen_worker_imports_generated_bindgen_module_once() {
    let worker = include_str!("../../../platform/src/os/web/web_worker.js");

    assert!(worker.contains(
        "const exports = await init({ module_or_path: thread_info.module, memory: env.memory }, env);"
    ));
    assert!(worker.contains("await doit({ exports });"));
    assert!(worker.contains("wasm.exports.__wbindgen_start()"));
}
