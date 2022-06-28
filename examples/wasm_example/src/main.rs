use makepad_wasm_msg::*;

#[derive(Debug, ToWasm)]
struct SysMouseInput {
    x: u32,
    y: u32
}

#[export_name = "process_to_wasm"]
#[cfg(target_arch = "wasm32")]
pub unsafe extern "C" fn process_to_wasm(msg_ptr: u32) -> u32 {
    let from_wasm_msg = FromWasmMsg::new();
    let mut to_wasm_msg = ToWasmMsg::from_wasm_ptr(msg_ptr);
    
    let cmd_id = LiveId(to_wasm_msg.read_u64());
    let _cmd_len = to_wasm_msg.read_u32();
    match cmd_id{
        id!(SysMouseInput)=>{
            let inp = SysMouseInput::to_wasm(&mut to_wasm_msg);
            console_log(&format!("{:?}", inp));
        }
        _=>()
    } 
    
    from_wasm_msg.into_wasm_ptr()
}



#[export_name = "get_wasm_msg_interface"]
#[cfg(target_arch = "wasm32")]
pub unsafe extern "C" fn get_wasm_msg_interface() -> u32 {
    let mut msg = FromWasmMsg::new();
    let mut out = String::new();
   
    SysMouseInput::codegen_js_method(&mut out, "SysMouseInput");
    
    msg.push_str(&out);
    msg.into_wasm_ptr()
}

fn main() {
    // in the decode for ID's we don't know we jump into the eventloop
    // MouseInput::to_wasm(towasmthing);
    // ok so the wasm IN table
    // how do we do this
    // ok now what
    // how do we tell JS the ToWasm table
    // it needs to be user-extendible
}
