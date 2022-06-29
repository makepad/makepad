use makepad_wasm_msg::*;

#[derive(Debug, ToWasm)]
struct SysMouseInput {
    x: u32,
    y: u32,
}

#[derive(Debug, FromWasm)]
struct ReturnMsg{
    x:u32,
    y:u32
}

#[export_name = "process_to_wasm"]
#[cfg(target_arch = "wasm32")]
pub unsafe extern "C" fn process_to_wasm(msg_ptr: u32) -> u32 {
    let mut from_wasm_msg = FromWasmMsg::new();
    let mut to_wasm_msg = ToWasmMsg::from_wasm_ptr(msg_ptr);
    
    while !to_wasm_msg.was_last_cmd(){
        let cmd_id = LiveId(to_wasm_msg.read_u64());
        let cmd_skip = to_wasm_msg.read_cmd_skip();
        match cmd_id{
            id!(SysMouseInput)=>{
                let inp = SysMouseInput::to_wasm(&mut to_wasm_msg);
                console_log!("{:?}", inp);
                ReturnMsg{x:2,y:3}.from_wasm(&mut from_wasm_msg);
                ReturnMsg{x:4,y:5}.from_wasm(&mut from_wasm_msg);
            }
            _=>()
        }
        to_wasm_msg.cmd_skip(cmd_skip);
        // skip over the command by cmd_len
        //console_log(&format!("{}", cmd_len));
    }
    from_wasm_msg.into_wasm_ptr()
}

#[export_name = "get_wasm_js_msg_impl"]
#[cfg(target_arch = "wasm32")]
pub unsafe extern "C" fn get_wasm_js_msg_impl() -> u32 {
    let mut msg = FromWasmMsg::new();
    let mut out = String::new();
   
    out.push_str("return {");
    out.push_str("   ToWasmMsg:class extends ToWasmMsg{");
    SysMouseInput::to_wasm_js_method(&mut out);
    out.push_str("   },");
    out.push_str("   FromWasmMsg:class extends FromWasmMsg{");
    ReturnMsg::from_wasm_js_method(&mut out);
    out.push_str("   }");
    out.push_str("}");
    
    msg.push_str(&out);
    msg.into_wasm_ptr()
}

fn main() {
}
