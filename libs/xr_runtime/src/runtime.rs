//! OpenXR loader entry point. Runtime functions are obtained through the negotiated
//! dispatch table; no global loader symbols are interposed into a game.
use crate::abi::*;

#[no_mangle]
pub unsafe extern "system" fn xrNegotiateLoaderRuntimeInterface(info: *const LoaderInfo, request: *mut RuntimeRequest) -> Result {
    if info.is_null() || request.is_null() { return INITIALIZATION_FAILED; }
    let (info, request) = (&*info, &mut *request);
    if info.ty != 1 || info.version != 1 || info.size != std::mem::size_of::<LoaderInfo>()
        || request.ty != 3 || request.version != 1 || request.size != std::mem::size_of::<RuntimeRequest>()
        || info.min_interface > 1 || info.max_interface < 1
        || info.min_api > API_VERSION || info.max_api < API_VERSION { return INITIALIZATION_FAILED; }
    request.interface = 1;
    request.api = API_VERSION;
    request.get_instance_proc_addr = Some(get_instance_proc_addr);
    SUCCESS
}

mod core;
mod state;
mod gpu;
use core::get_instance_proc_addr;
