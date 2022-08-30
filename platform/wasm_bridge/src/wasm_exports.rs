use crate::from_wasm::*;
use crate::to_wasm::*;
use crate::wasm_types::*;

#[export_name = "wasm_new_msg_with_u64_capacity"]
#[cfg(target_arch = "wasm32")]
pub unsafe extern "C" fn wasm_new_msg_with_u64_capacity(capacity_u64: u32) -> u32 {
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

#[export_name = "wasm_new_data_u8"]
#[cfg(target_arch = "wasm32")]
pub unsafe extern "C" fn wasm_new_data_u8(capacity_u8: u32) -> u32 {
    WasmDataU8::new_and_release_ownership(capacity_u8 as usize)
}

#[export_name = "wasm_free_data_u8"]
#[cfg(target_arch = "wasm32")]
pub unsafe extern "C" fn wasm_free_data_u8(ptr: u32, len:u32, cap:u32) {
    WasmDataU8::take_ownership(ptr, len, cap);
}
