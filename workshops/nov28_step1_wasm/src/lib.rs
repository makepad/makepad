 
#[export_name = "wasm_hello_world"]
pub extern "C" fn wasm_hello_world(input:u32) -> u32 {
    return input + 1
} 