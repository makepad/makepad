use makepad_wasm_msg::*;

#[derive(Debug, FromWasm, ToWasm)]
struct BridgeTest {
    u1: u32,
    b1: bool,
    e1: EnumTest,
    e2: EnumTest,
    e3: EnumTest,
    e4: EnumTest,
    o1: Option<u32>,
    o2: Option<u32>,
    r1: Vec<RecurTest>
}

#[derive(Debug, FromWasm, ToWasm)]
struct RecurTest{
    u2: u32,
    b2: bool,
    r2: Vec<RecurTest>
}

#[derive(Debug, FromWasm, ToWasm)]
enum EnumTest {
    Bare,
    Tuple(u32),
    Recur(Vec<EnumTest>),
    Named{x:u32}
}

fn create_test()->BridgeTest{
    BridgeTest{
        u1: 1,
        b1: true,
        e1: EnumTest::Bare,
        e2: EnumTest::Tuple(2),
        e3: EnumTest::Recur(vec!{EnumTest::Bare}),
        e4: EnumTest::Named{x:3},
        o1: None,
        o2: Some(4),
        r1: vec![RecurTest{
            u2: 5,
            b2: false,
            r2: vec![RecurTest{
                u2: 5,
                b2: false,
                r2: vec![RecurTest{
                    u2:6,
                    b2: false,
                    r2: vec![]
                }]
            }]
        }]
    }
}

#[derive(Debug, ToWasm)]
struct InitTest{
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
            id!(InitTest)=>{
                let test = create_test();
                test.from_wasm(&mut from_wasm);
            },
            id!(BridgeTest)=>{
                let test1 = create_test();
                let test2 = BridgeTest::to_wasm(&mut to_wasm);
            }
            _=>()
        }
        to_wasm.cmd_skip(cmd_skip);
    }
    from_wasm.into_wasm_ptr()
}

#[export_name = "get_wasm_js_msg_impl"]
#[cfg(target_arch = "wasm32")]
pub unsafe extern "C" fn get_wasm_js_msg_impl() -> u32 {
    let mut msg = FromWasmMsg::new();
    let mut out = String::new();
   
    out.push_str("return {\n");
    out.push_str("ToWasmMsg:class extends ToWasmMsg{\n");
    InitTest::to_wasm_js_method(&mut out);
    BridgeTest::to_wasm_js_method(&mut out);
    out.push_str("},\n");
    out.push_str("FromWasmMsg:class extends FromWasmMsg{\n");
    BridgeTest::from_wasm_js_method(&mut out);
    out.push_str("}\n");
    out.push_str("}");
    
    msg.push_str(&out);
    msg.into_wasm_ptr()
}

fn main() {
}
