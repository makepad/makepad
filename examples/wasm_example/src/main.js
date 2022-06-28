import {WasmAppWebGL} from "/makepad/platform/src/platform/webbrowser/webgl_platform.js"

let canvas = document.getElementsByClassName('main_canvas')[0];

WasmAppWebGL.create_from_wasm_url(
    "/makepad/target/wasm32-unknown-unknown/debug/wasm_example.wasm",
    canvas,
    (app) => {
        // ok great. lets send the wasmblob a message
        let msg = new app.ToWasmMsg(app);
        msg.SysMouseInput({x:1234,y:5432});
        let ret = app.process_to_wasm(msg.finalise());
    },
    (err) => {
    }
);

