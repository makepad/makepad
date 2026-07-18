const WEB_JS: &str = include_str!("../src/os/web/web.js");

#[test]
fn web_perf_snapshot_splits_wasm_and_dispatch_time() {
    assert!(WEB_JS.contains("last_wasm_ms: 0,"));
    assert!(WEB_JS.contains("last_dispatch_ms: 0,"));

    let snapshot = WEB_JS
        .split("get_perf_snapshot() {")
        .nth(1)
        .unwrap()
        .split("perf_snapshot_is_active")
        .next()
        .unwrap();
    assert!(snapshot.contains("wasm_ms: perf.last_wasm_ms || 0,"));
    assert!(snapshot.contains("dispatch_ms: perf.last_dispatch_ms || 0,"));

    let pump = WEB_JS
        .split("do_wasm_pump() {")
        .nth(1)
        .unwrap()
        .split("ensure_perf_hud()")
        .next()
        .unwrap();
    assert!(pump.contains("wasm_ms: this.perf.last_wasm_ms,"));
    assert!(pump.contains("dispatch_ms: this.perf.last_dispatch_ms,"));
    let last_active = pump
        .split("const active_snapshot = {")
        .nth(1)
        .unwrap()
        .split("};")
        .next()
        .unwrap();
    assert!(last_active.contains("wasm_ms: snapshot.wasm_ms,"));
    assert!(last_active.contains("dispatch_ms: snapshot.dispatch_ms,"));

    let hud = WEB_JS
        .split("update_perf_hud() {")
        .nth(1)
        .unwrap()
        .split("wasm_process_msg(to_wasm)")
        .next()
        .unwrap();
    assert!(hud.contains("this.perf.last_wasm_ms.toFixed(2)"));
    assert!(hud.contains("this.perf.last_dispatch_ms.toFixed(2)"));

    let wasm_started = pump.find("const wasm_started = performance.now();").unwrap();
    let wasm = pump.find("this.wasm_process_msg(to_wasm)").unwrap();
    let wasm_finished = pump
        .find("const wasm_ms = performance.now() - wasm_started;")
        .unwrap();
    let dispatch_started = pump
        .find("const dispatch_started = performance.now();")
        .unwrap();
    let dispatch = pump.find("from_wasm.dispatch_on_app()").unwrap();
    let dispatch_finished = pump
        .find("dispatch_ms = performance.now() - dispatch_started;")
        .unwrap();
    let free = pump.find("from_wasm.free()").unwrap();
    let pump_finished = pump
        .find("const pump_ms = performance.now() - started;")
        .unwrap();
    let wasm_assigned = pump.find("this.perf.last_wasm_ms = wasm_ms;").unwrap();
    let dispatch_assigned = pump
        .find("this.perf.last_dispatch_ms = dispatch_ms;")
        .unwrap();
    let published = pump.find("const snapshot = {").unwrap();
    assert!(
        wasm_started < wasm
            && wasm < wasm_finished
            && wasm_finished < dispatch_started
            && dispatch_started < dispatch
            && dispatch < dispatch_finished
            && dispatch_finished < free
            && free < pump_finished
            && pump_finished < wasm_assigned
            && wasm_assigned < dispatch_assigned
            && dispatch_assigned < published
    );

    let dispatch_timing = &pump[dispatch_started..free];
    assert!(dispatch_timing.contains(
        "try {\n                from_wasm.dispatch_on_app();\n            }\n            finally {\n                dispatch_ms = performance.now() - dispatch_started;\n            }"
    ));
}
