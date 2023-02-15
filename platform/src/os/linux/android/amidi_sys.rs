#![allow(non_camel_case_types)]

use self::super::jni_sys::*;
use std::os::raw::c_long;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct AMidiDevice {
    _unused: [u8; 0],
}

pub type media_status_t = std::os::raw::c_int;

#[link(name = "amidi")]
extern "C" {
    pub fn AMidiDevice_fromJava(
        env: *mut JNIEnv,
        midiDeviceObj: jobject,
        outDevicePtrPtr: *mut *mut AMidiDevice,
    ) -> media_status_t;
    
    pub fn AMidiDevice_getNumInputPorts(device: *const AMidiDevice) -> c_long;
    pub fn AMidiDevice_getNumOutputPorts(device: *const AMidiDevice) -> c_long;
    
}