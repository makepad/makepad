import {WasmApp} from "../../src/wasm_app.js"

class MyWasmApp extends WasmApp {
    BridgeTest(obj) {
        console.log("BridgeTest arrived", obj)
        let to_wasm = app.new_to_wasm();
        to_wasm.BridgeTest(obj);
        app.do_wasm_pump(to_wasm);
    }
    
    do_wasm_pump(to_wasm) {
        let ret_ptr = this.exports.wasm_process_msg(to_wasm.release_ownership());
        let from_wasm = this.new_from_wasm(ret_ptr);
        from_wasm.dispatch_on_app();
        from_wasm.free();
    }
}

const wasm = await MyWasmApp.load_wasm_from_url("/makepad/target/wasm32-unknown-unknown/debug/wasm_bridge_test.wasm"); 

let app = new MyWasmApp(wasm);

console.log(app.msg_class)

let to_wasm = app.new_to_wasm();
to_wasm.RunTest();
app.do_wasm_pump(to_wasm);