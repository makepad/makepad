#![allow(non_camel_case_types)]
use makepad_objc_sys::runtime::ObjcId;
use std::ffi::c_void;
use std::ffi::c_char;
type nw_parameters_t = ObjcId;
type nw_endpoint_t = ObjcId;
type nw_connection_t = ObjcId;
type dispatch_queue_t = ObjcId;
type nw_parameters_configure_protocol_block_t = ObjcId;
type nw_connection_state_changed_handler_t = ObjcId;

#[repr(u32)]
#[derive(Debug)]
pub enum nw_connection_state {
    invalid = 0,
    waiting = 1,
    preparing = 2,
    ready = 3,
    failed = 4,
    cancelled = 5,
}

#[link(name = "system")]
extern {
    pub static _NSConcreteStackBlock: [*const c_void; 32];
    pub static _NSConcreteBogusBlock: [*const c_void; 32];
}

#[link(name = "Foundation", kind = "framework")]
extern "C" {
    pub fn dispatch_queue_create(label: *const u8, attr: ObjcId,) -> dispatch_queue_t;
    pub fn dispatch_release(object: dispatch_queue_t);
}

#[link(name = "Network", kind = "framework")]
extern "C" {
    
    pub static _nw_parameters_configure_protocol_default_configuration: ObjcId;
    pub fn nw_parameters_create_secure_tcp(
        configure_dtls: nw_parameters_configure_protocol_block_t,
        configure_udp: nw_parameters_configure_protocol_block_t
    ) -> nw_parameters_t;
    pub fn nw_endpoint_create_host(
        hostname: *const c_char,
        port: *const c_char
    ) -> nw_endpoint_t;
    
    pub fn nw_connection_create(
        endpoint: nw_endpoint_t,
        parameters: nw_parameters_t
    ) -> nw_connection_t;
    pub fn nw_connection_set_queue(connection: nw_connection_t, queue: dispatch_queue_t);
    
    pub fn nw_connection_set_state_changed_handler(
        connection: nw_connection_t,
        handler: nw_connection_state_changed_handler_t
    );
}
