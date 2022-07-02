import {WebGLWasmApp} from "/makepad/platform/src/platform/webbrowser/webgl_platform.js"

const canvas = document.getElementsByClassName('main_canvas')[0];

class MyWasmApp extends WebGLWasmApp {
    ReturnMsg(obj) {
        console.log("ReturnMsg arrived", obj)
    }
    EnumTest(obj){
        console.log("EnumTest arrived", obj);
    }
}

const wasm = await MyWasmApp.load_wasm_from_url("/makepad/target/wasm32-unknown-unknown/debug/wasm_example.wasm");

let app = new MyWasmApp(canvas, wasm);

let to_wasm = app.new_to_wasm();
to_wasm.SysMouseInput({x: 1234, y: [{type:"Tuple",0:1294},{type:"Named",x:12}]});
to_wasm.SysMouseInput({x: 1511, y: [{type:"Named",x:12345}]});

app.to_wasm_pump(to_wasm);