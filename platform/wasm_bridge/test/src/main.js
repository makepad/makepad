import {WasmApp} from "../../src/wasm_app.js"

class MyWasmApp extends WasmApp {
    BridgeTest(obj) {
        console.log("BridgeTest arrived", obj)
        let to_wasm = app.new_to_wasm();
        to_wasm.BridgeTest(obj);
        app.to_wasm_pump(to_wasm);
    }
}

const wasm = await MyWasmApp.load_wasm_from_url("/makepad/target/wasm32-unknown-unknown/debug/wasm_example.wasm"); 

let app = new MyWasmApp(wasm);

console.log(app.msg_class)

let to_wasm = app.new_to_wasm();
to_wasm.RunTest();
app.to_wasm_pump(to_wasm);