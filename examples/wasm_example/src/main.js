import {WebGLWasmApp} from "/makepad/platform/src/platform/webbrowser/webgl_platform.js"

let canvas = document.getElementsByClassName('main_canvas')[0];

class MyWasmApp extends WebGLWasmApp {
    ReturnMsg(obj) {
    }
}

MyWasmApp.load_wasm_from_url(
    "/makepad/target/wasm32-unknown-unknown/debug/wasm_example.wasm",
    (wasm) => {
        let app = new MyWasmApp(canvas, wasm);
        let to_wasm = new app.msg_class.ToWasmMsg(app);
        to_wasm.SysMouseInput({x: 1234, y: 5432});
        let ret_ptr = app.process_to_wasm(to_wasm.finalise());
        let from_wasm = new app.msg_class.FromWasmMsg(app, ret_ptr);
        from_wasm.dispatch();
        from_wasm.destroy();
    },
    (err) => {
    }
);

