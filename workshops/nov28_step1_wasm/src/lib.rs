 
#[export_name = "wasm_hello_world"]
pub extern "C" fn wasm_hello_world(amount:u32) -> u32 {
    // this is the data we are returning
    let mut data = Vec::new();
    for i in 0..amount{
        data.push(i as f32)
    }
    let data = std::mem::ManuallyDrop::new(data);

    let ret = vec![data.as_ptr() as u32, data.len() as u32];
    let ret = std::mem::ManuallyDrop::new(ret);
    return ret.as_ptr() as u32;
}    