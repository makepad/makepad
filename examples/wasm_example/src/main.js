import {WebGLWasmApp} from "/makepad/platform/src/platform/webbrowser/webgl_platform.js"

const canvas = document.getElementsByClassName('main_canvas')[0];

class MyWasmApp extends WebGLWasmApp {
    BridgeTest(obj) {
        console.log("BridgeTest arrived", obj)
        let to_wasm = app.new_to_wasm();
        to_wasm.BridgeTest(obj);
        app.to_wasm_pump(to_wasm);
    }
}

const wasm = await MyWasmApp.load_wasm_from_url("/makepad/target/wasm32-unknown-unknown/debug/wasm_example.wasm");

let app = new MyWasmApp(canvas, wasm);

let to_wasm = app.new_to_wasm();
to_wasm.InitTest();
app.to_wasm_pump(to_wasm);