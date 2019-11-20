function main() {
    // lets download the wasm file
    fetch("/makepad/target/wasm32-unknown-unknown/debug/nov28_step1_wasm.wasm")
        .then(response => response.arrayBuffer())
        .then(bytes => WebAssembly.instantiate(bytes, {}))
        .then(results => {
            main_wasm(results);
    }, errors => {
        console.log("Error compiling wasm file");
    });
}
    

function main_wasm(wasm){
    console.log("The wasm object", wasm);

    // lets call the wasm function
    let ret = wasm.instance.exports.wasm_hello_world(10);

    let mem = wasm.instance.exports.memory.buffer;
    
    // ret is now a pointer
    let pair = new Uint32Array(mem, ret, 2);
    let buf = new Float32Array(mem, pair[0], pair[1]);
    console.log(buf);
}