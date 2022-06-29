import {WebGLWasmApp} from "/makepad/platform/src/platform/webbrowser/webgl_platform.js"

let canvas = document.getElementsByClassName('main_canvas')[0];

class MyWasmApp extends WebGLWasmApp {
    ReturnMsg(obj) {
        console.log("ReturnMsg arrived", obj)
    }
}

MyWasmApp.load_wasm_from_url(
    "/makepad/target/wasm32-unknown-unknown/debug/wasm_example.wasm",
    (wasm) => {
        let app = new MyWasmApp(canvas, wasm);
        
        let to_wasm = app.new_to_wasm();
        to_wasm.SysMouseInput({x: 1234, y: 5432});
        to_wasm.SysMouseInput({x: 1511, y: 1518});
        
        app.to_wasm_pump(to_wasm);
    },
    (err) => {
    }
);

