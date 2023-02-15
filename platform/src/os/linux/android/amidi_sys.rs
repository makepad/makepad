#![allow(non_camel_case_types)]

use self::super::jni_sys::*;
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct AMidiDevice {
    _unused: [u8; 0],
}

pub type media_status_t = std::os::raw::c_int;

#[link(name = "amidi")]
extern "C"{
    pub fn AMidiDevice_fromJava(
        env: *mut JNIEnv,
        midiDeviceObj: jobject,
        outDevicePtrPtr: *mut *mut AMidiDevice,
    ) -> media_status_t;
}