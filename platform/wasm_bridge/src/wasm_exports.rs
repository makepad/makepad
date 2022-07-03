use crate::from_wasm::*;
use crate::to_wasm::*;
use crate::wasm_types::*;

#[export_name = "new_wasm_msg_with_u64_capacity"]
#[cfg(target_arch = "wasm32")]
pub unsafe extern "C" fn new_wasm_msg_with_u64_capacity(capacity_u64: u32) -> u32 {
    FromWasmMsg::new().reserve_u64(capacity_u64 as usize).release_ownership()
}

#[export_name = "wasm_msg_reserve_u64"]
#[cfg(target_arch = "wasm32")]
pub unsafe extern "C" fn wasm_msg_reserve_u64(ptr: u32, capacity_u64: u32) -> u32 {
    ToWasmMsg::take_ownership(ptr).into_from_wasm().reserve_u64(capacity_u64 as usize).release_ownership()
}

#[export_name = "wasm_msg_free"]
#[cfg(target_arch = "wasm32")]
pub unsafe extern "C" fn wasm_msg_free(ptr: u32) {
    ToWasmMsg::take_ownership(ptr);
}

#[export_name = "new_to_wasm_data_u8"]
#[cfg(target_arch = "wasm32")]
pub unsafe extern "C" fn new_to_wasm_data_u8(capacity_u8: u32) -> u32 {
    ToWasmDataU8::new_and_release_ownership(capacity_u8 as usize)
}

extern "C" {
    pub fn _console_log(chars: u32, len: u32);
}

pub fn console_log_impl(val: &str) {
    unsafe {
        let chars = val.chars().collect::<Vec<char >> ();
        _console_log(chars.as_ptr() as u32, chars.len() as u32);
    }
}

#[macro_export]
macro_rules!console_log {
    ( $ ( $ t: tt) *) => {
        console_log_impl(&format!("{}:{} - {}", file!(), line!(), format!( $ ( $ t) *)))
    }
}
