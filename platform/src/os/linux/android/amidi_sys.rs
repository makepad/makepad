#![allow(non_camel_case_types)]

use self::super::jni_sys::*;
use std::os::raw::{c_long,c_ulong};

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct AMidiDevice {
    _unused: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct AMidiInputPort {
    _unused: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct AMidiOutputPort {
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
    pub fn AMidiDevice_release(midiDevice: *const AMidiDevice) -> media_status_t;
    
    pub fn AMidiDevice_getNumInputPorts(device: *const AMidiDevice) -> c_long;
    pub fn AMidiDevice_getNumOutputPorts(device: *const AMidiDevice) -> c_long;
    pub fn AMidiOutputPort_open(
        device: *const AMidiDevice,
        portNumber: i32,
        outOutputPortPtr: *mut *mut AMidiOutputPort,
    ) -> media_status_t;
    pub fn AMidiOutputPort_close(outputPort: *const AMidiOutputPort);
    pub fn AMidiInputPort_open(
        device: *const AMidiDevice,
        portNumber: i32,
        outInputPortPtr: *mut *mut AMidiInputPort,
    ) -> media_status_t;
    pub fn AMidiInputPort_send(
        inputPort: *const AMidiInputPort,
        buffer: *const u8,
        numBytes: c_ulong,
    ) -> c_long;
    pub fn AMidiInputPort_close(inputPort: *const AMidiInputPort);
    pub fn AMidiOutputPort_receive(
        outputPort: *const AMidiOutputPort,
        opcodePtr: *mut i32,
        buffer: *mut u8,
        maxBytes: c_ulong,
        numBytesReceivedPtr: *mut c_ulong,
        outTimestampPtr: *mut i64,
    ) -> c_long;    
}