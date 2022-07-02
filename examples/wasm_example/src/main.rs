use makepad_wasm_msg::*;

#[derive(Debug, ToWasm)]
struct SysMouseInput {
    x: u32,
    y: Vec<EnumTest>,
}

#[derive(Debug, ToWasm, FromWasm)]
struct SubObj {
    a:u32,
    b:u32
}

#[derive(Debug, FromWasm, ToWasm)]
enum EnumTest {
    Bare,
    Tuple(u32),
    Named{x:u32}
}

#[derive(Debug, FromWasm)]
struct ReturnMsg{
    x:u32,
    y:Vec<SubObj>
}

#[export_name = "process_to_wasm_msg"]
#[cfg(target_arch = "wasm32")]
pub unsafe extern "C" fn process_to_wasm_msg(msg_ptr: u32) -> u32 {
    let mut from_wasm = FromWasmMsg::new();
    let mut to_wasm = ToWasmMsg::from_wasm_ptr(msg_ptr);
    
    while !to_wasm.was_last_cmd(){
        let cmd_id = LiveId(to_wasm.read_u64());
        let cmd_skip = to_wasm.read_cmd_skip();
        match cmd_id{
            id!(SysMouseInput)=>{
                let inp = SysMouseInput::to_wasm(&mut to_wasm);
                console_log!("{:?}", inp);
                ReturnMsg{x:2,y:vec![SubObj{a:3,b:4}]}.from_wasm(&mut from_wasm);
                EnumTest::Bare.from_wasm(&mut from_wasm);
                EnumTest::Tuple(inp.x).from_wasm(&mut from_wasm);
                EnumTest::Named{x:456}.from_wasm(&mut from_wasm);
            }
            _=>()
        }
        to_wasm.cmd_skip(cmd_skip);
        // skip over the command by cmd_len
        //console_log(&format!("{}", cmd_len));
    }
    from_wasm.into_wasm_ptr()
}

#[export_name = "get_wasm_js_msg_impl"]
#[cfg(target_arch = "wasm32")]
pub unsafe extern "C" fn get_wasm_js_msg_impl() -> u32 {
    let mut msg = FromWasmMsg::new();
    let mut out = String::new();
   
    out.push_str("return {");
    out.push_str("   ToWasmMsg:class extends ToWasmMsg{");
    SysMouseInput::to_wasm_js_method(&mut out);
    EnumTest::to_wasm_js_method(&mut out);
    out.push_str("   },");
    out.push_str("   FromWasmMsg:class extends FromWasmMsg{");
    ReturnMsg::from_wasm_js_method(&mut out);
    EnumTest::from_wasm_js_method(&mut out);
    out.push_str("   }");
    out.push_str("}");
    
    msg.push_str(&out);
    msg.into_wasm_ptr()
}

fn main() {
}
