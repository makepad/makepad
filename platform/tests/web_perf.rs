const WEB_JS: &str = include_str!("../src/os/web/web.js");
const WEB_GL_JS: &str = include_str!("../src/os/web/web_gl.js");

fn braced_body<'a>(source: &'a str, marker: &str) -> &'a str {
    let marker_at = source.find(marker).unwrap();
    let open = marker_at + source[marker_at..].find('{').unwrap();
    let mut depth = 0;
    for (offset, byte) in source.as_bytes()[open..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[open + 1..open + offset];
                }
            }
            _ => {}
        }
    }
    panic!("unclosed body for {marker}");
}

fn assert_each_call_followed_by(body: &str, call: &str, counter: &str, expected: usize) {
    let calls: Vec<_> = body.match_indices(call).map(|(at, _)| at).collect();
    let counters: Vec<_> = body.match_indices(counter).map(|(at, _)| at).collect();
    assert_eq!(calls.len(), expected);
    assert_eq!(counters.len(), expected);
    for index in 0..expected {
        assert!(calls[index] < counters[index]);
        if index + 1 < expected {
            assert!(counters[index] < calls[index + 1]);
        }
    }
}

fn object_keys(object: &str) -> Vec<&str> {
    object
        .lines()
        .filter_map(|line| line.trim().split_once(':').map(|(key, _)| key))
        .collect()
}

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

#[test]
fn webgl_perf_snapshot_counts_backend_work() {
    const FIELDS: [&str; 9] = [
        "passes",
        "draw_commands",
        "submits",
        "uniform_write_calls",
        "uniform_write_bytes",
        "buffer_write_calls",
        "buffer_write_bytes",
        "texture_write_calls",
        "texture_write_bytes",
    ];

    for hook in [
        "reset_backend_perf() {",
        "get_backend_perf_snapshot() {",
        "format_backend_perf_hud() {",
    ] {
        assert!(WEB_GL_JS.contains(hook), "missing {hook}");
    }

    let reset = braced_body(WEB_GL_JS, "reset_backend_perf() {");
    let reset_object = braced_body(reset, "this._backend_perf = {");
    let getter = braced_body(WEB_GL_JS, "get_backend_perf_snapshot() {");
    let snapshot_object = braced_body(getter, "return {");
    assert_eq!(object_keys(reset_object), FIELDS);
    assert_eq!(object_keys(snapshot_object), FIELDS);
    for field in FIELDS {
        assert!(reset_object.contains(&format!("{field}: 0,")));
        assert!(reset.contains(&format!("perf.{field} = 0;")));
        assert!(snapshot_object.contains(&format!("{field}: perf.{field} || 0,")));
    }
    assert!(reset.contains("let perf = this._backend_perf;"));
    assert!(reset.contains("if (!perf) {"));
    assert!(getter.contains("const perf = this._backend_perf || {};"));
    assert!(!getter.contains("return this._backend_perf"));

    let constructor = braced_body(WEB_GL_JS, "constructor(wasm, dispatch, canvas) {");
    let super_call = constructor.find("super(wasm, dispatch, canvas);").unwrap();
    let initial_reset = constructor.find("this.reset_backend_perf();").unwrap();
    let undefined_return = constructor.find("if (wasm === undefined)").unwrap();
    assert!(super_call < initial_reset && initial_reset < undefined_return);

    let hud = braced_body(WEB_GL_JS, "format_backend_perf_hud() {");
    for field in FIELDS {
        assert!(hud.contains(&format!("perf.{field}")));
    }

    for pass in ["FromWasmBeginRenderTexture(args) {", "FromWasmBeginRenderCanvas(args) {"] {
        let body = braced_body(WEB_GL_JS, pass);
        assert_eq!(body.matches("this._backend_perf.passes += 1;").count(), 1);
    }

    let command_buffer = braced_body(WEB_GL_JS, "FromWasmRenderCommandBuffer(args) {");
    let decoded_draw = &command_buffer[..command_buffer.find("const shader_id").unwrap()];
    assert!(decoded_draw.contains("cmd !== CMD_DRAW"));
    assert!(decoded_draw.contains("this.perf.draw_calls = (this.perf.draw_calls | 0) + 1;"));
    assert_eq!(
        decoded_draw
            .matches("this._backend_perf.draw_commands += 1;")
            .count(),
        1
    );

    let draw_call = braced_body(WEB_GL_JS, "FromWasmDrawCall(args) {");
    let submit_call = "gl.drawElementsInstanced(";
    let submit_counter = "this._backend_perf.submits += 1;";
    assert_each_call_followed_by(draw_call, submit_call, submit_counter, 3);
    assert_each_call_followed_by(command_buffer, submit_call, submit_counter, 3);
    assert_eq!(WEB_GL_JS.matches(submit_call).count(), 6);

    let ptr_uniform = braced_body(WEB_GL_JS, "upload_uniform_buffer_from_ptr(gl, gl_buf, ptr_f32) {");
    let cache_skip = ptr_uniform.find("_last_upload_memory_byte_length").unwrap();
    let uniform_data = ptr_uniform.find("gl.bufferData(gl.UNIFORM_BUFFER").unwrap();
    let uniform_sub_data = ptr_uniform.find("gl.bufferSubData(gl.UNIFORM_BUFFER").unwrap();
    let uniform_count = ptr_uniform
        .find("this._backend_perf.uniform_write_calls += 1;")
        .unwrap();
    assert!(cache_skip < uniform_data && uniform_data < uniform_sub_data && uniform_sub_data < uniform_count);
    assert!(ptr_uniform[uniform_count..]
        .contains("this._backend_perf.uniform_write_bytes += byte_length;"));

    let generic_uniform = braced_body(WEB_GL_JS, "upload_uniform_buffer_data(gl, gl_buf, data");
    assert!(!generic_uniform.contains("_backend_perf"));
    let generic_write = braced_body(WEB_GL_JS, "upload_buffer_data(gl, target, gl_buf, data, usage) {");
    let generic_data = generic_write.find("gl.bufferData(target").unwrap();
    let generic_sub_data = generic_write.find("gl.bufferSubData(target").unwrap();
    let categorize = generic_write.find("target === gl.UNIFORM_BUFFER").unwrap();
    assert!(generic_data < generic_sub_data && generic_sub_data < categorize);
    for update in [
        "this._backend_perf.uniform_write_calls += 1;",
        "this._backend_perf.uniform_write_bytes += byte_length;",
        "this._backend_perf.buffer_write_calls += 1;",
        "this._backend_perf.buffer_write_bytes += byte_length;",
    ] {
        assert_eq!(generic_write.matches(update).count(), 1);
    }
    for allocation in ["FromWasmAllocIndexBuffer(args) {", "FromWasmAllocArrayBuffer(args) {"] {
        assert!(braced_body(WEB_GL_JS, allocation).contains("this.upload_buffer_data("));
    }

    for upload in [
        "FromWasmAllocTextureImage2D_BGRAu8_32(args) {",
        "FromWasmAllocTextureImage2D_Ru8(args) {",
        "FromWasmAllocTextureImage2D_RGBAf32(args) {",
    ] {
        let body = braced_body(WEB_GL_JS, upload);
        assert_each_call_followed_by(
            body,
            "gl.texImage2D(",
            "this._backend_perf.texture_write_calls += 1;",
            1,
        );
        assert_each_call_followed_by(
            body,
            "gl.texImage2D(",
            "this._backend_perf.texture_write_bytes += data_array.byteLength;",
            1,
        );
    }
    let cube = braced_body(WEB_GL_JS, "FromWasmAllocTextureCube_BGRAu8_32(args) {");
    assert!(cube.contains("for (let i = 0; i < 6; i++)"));
    let cube_loop = braced_body(cube, "for (let i = 0; i < 6; i++) {");
    assert_each_call_followed_by(
        cube_loop,
        "gl.texImage2D(",
        "this._backend_perf.texture_write_calls += 1;",
        1,
    );
    assert_each_call_followed_by(
        cube_loop,
        "gl.texImage2D(",
        "this._backend_perf.texture_write_bytes += data_array.byteLength;",
        1,
    );

    let render_target = braced_body(WEB_GL_JS, "FromWasmBeginRenderTexture(args) {");
    assert!(render_target.contains("null,"));
    assert!(!render_target.contains("texture_write_"));
    let video = braced_body(WEB_GL_JS, "update_video_texture(player) {");
    assert!(video.contains("gl.texImage2D("));
    assert!(!video.contains("texture_write_"));
}
