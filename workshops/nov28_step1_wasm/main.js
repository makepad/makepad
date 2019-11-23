// download the wasm file
function main() {
    fetch("/makepad/target/wasm32-unknown-unknown/debug/nov28_step1_wasm.wasm")
        .then(response => response.arrayBuffer())
        .then(bytes => WebAssembly.instantiate(bytes, {}))
        .then(results => {
        main_wasm(results);
    }, errors => {
        console.log("Error compiling wasm file");
    });
}

// take a return from wasm and 
function grab_float_vec(rust, ptr) {
    let mem = rust.memory.buffer;
    let to_js = new Uint32Array(mem, ptr, 3);
    let buf = new Float32Array(mem, to_js[0], to_js[1]);
    return {ptr, to_js, buf}
}

function main_wasm(wasm) {
    console.log("The wasm object", wasm);
    let rust = wasm.instance.exports;
    // lets call the wasm function
    let vec = grab_float_vec(rust, rust.hello_world(10));
    console.log("hello")
    console.log(vec.buf);
    
    // free it again
    rust.free_float_vec(vec.ptr);
}