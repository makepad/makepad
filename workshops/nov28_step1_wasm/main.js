function main_wasm(wasm) {
    console.log("Have Wasm", wasm);
    let ret = wasm.instance.exports.wasm_hello_world(10);
    console.log(ret);
}
