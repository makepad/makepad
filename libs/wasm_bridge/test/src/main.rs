#![allow(unused)]
#![cfg(target_arch = "wasm32")]
use makepad_wasm_bridge::*;

#[derive(Debug, FromWasm, ToWasm, PartialEq)]
struct BridgeTest {
    u1: u32,
    b1: bool,
    e1: EnumTest,
    e2: EnumTest,
    e3: EnumTest,
    e4: EnumTest,
    o1: Option<u32>,
    o2: Option<u32>,
    v1: [u32;2],
    r1: Vec<RecurTest>
}

#[derive(Debug, FromWasm, ToWasm, PartialEq)]
struct RecurTest {
    u2: u32,
    b2: bool,
    bx: Option<Box<RecurTest>>,
    r2: Vec<RecurTest>
}

#[derive(Debug, FromWasm, ToWasm, PartialEq)]
enum EnumTest {
    Bare,
    Tuple(u32),
    Recur(Vec<EnumTest>),
    Named {x: u32}
}

fn create_test() -> BridgeTest {
    BridgeTest {
        u1: 1,
        b1: true,
        v1: [1,2],
        e1: EnumTest::Bare,
        e2: EnumTest::Tuple(2),
        e3: EnumTest::Recur(vec!{EnumTest::Bare}),
        e4: EnumTest::Named {x: 3},
        o1: None,
        o2: Some(4),
        r1: vec![RecurTest {
            bx: None,
            u2: 5,
            b2: false,
            r2: vec![RecurTest {
                bx: None,
                u2: 6,
                b2: true,
                r2: vec![RecurTest {
                    bx: Some(Box::new(RecurTest {
                        bx: None,
                        u2: 6,
                        b2: true,
                        r2: vec![]
                    })),
                    u2: 7,
                    b2: false,
                    r2: vec![]
                }]
            }]
        }]
    }
}

#[derive(Debug, ToWasm)]
struct RunTest {
}

#[export_name = "wasm_process_msg"]
#[cfg(target_arch = "wasm32")]
pub unsafe extern "C" fn process_to_wasm_msg(msg_ptr: u32) -> u32 {
    let mut from_wasm = FromWasmMsg::new();
    let mut to_wasm = ToWasmMsg::take_ownership(msg_ptr);
    
    while !to_wasm.was_last_block() {
        let id = LiveId(to_wasm.read_u64());
        let skip = to_wasm.read_block_skip();
        match id {
            live_id!(RunTest) => {
                let test = create_test();
                test.write_from_wasm(&mut from_wasm);
            },
            live_id!(BridgeTest) => {
                let test1 = create_test();
                let test2 = BridgeTest::read_to_wasm(&mut to_wasm);
                if test1 == test2 {
                    console_log!("test_succeeded!");
                }
                else {
                    console_log!("test_failed! {:?}", test2);
                }
            }
            _ => ()
        }
        to_wasm.block_skip(skip);
    }
    from_wasm.release_ownership()
}

#[export_name = "wasm_get_js_msg_class"]
#[cfg(target_arch = "wasm32")]
pub unsafe extern "C" fn get_wasm_js_msg_class() -> u32 {
    let mut msg = FromWasmMsg::new();
    let mut out = String::new();
    
    out.push_str("return {\n");
    out.push_str("ToWasmMsg:class extends ToWasmMsg{\n");
    RunTest::to_wasm_js_method(&mut out);
    BridgeTest::to_wasm_js_method(&mut out);
    out.push_str("},\n");
    out.push_str("FromWasmMsg:class extends FromWasmMsg{\n");
    BridgeTest::from_wasm_js_method(&mut out);
    out.push_str("}\n");
    out.push_str("}");
    
    msg.push_str(&out);
    msg.release_ownership()
}

fn main() {
}
