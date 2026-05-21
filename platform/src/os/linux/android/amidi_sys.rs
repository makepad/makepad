#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
use jni_sys::*;
use makepad_jni_sys as jni_sys;
use std::{
    os::raw::{c_char, c_int, c_long, c_ulong, c_void},
    ptr,
    sync::OnceLock,
};

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

const AMEDIA_ERROR_UNKNOWN: media_status_t = -10000;
const RTLD_NOW: c_int = 2;
const RTLD_LOCAL: c_int = 0;

type AMidiDeviceFromJava =
    unsafe extern "C" fn(*mut JNIEnv, jobject, *mut *mut AMidiDevice) -> media_status_t;
type AMidiDeviceRelease = unsafe extern "C" fn(*const AMidiDevice) -> media_status_t;
type AMidiDeviceGetNumPorts = unsafe extern "C" fn(*const AMidiDevice) -> c_long;
type AMidiOutputPortOpen =
    unsafe extern "C" fn(*const AMidiDevice, i32, *mut *mut AMidiOutputPort) -> media_status_t;
type AMidiOutputPortClose = unsafe extern "C" fn(*const AMidiOutputPort);
type AMidiInputPortOpen =
    unsafe extern "C" fn(*const AMidiDevice, i32, *mut *mut AMidiInputPort) -> media_status_t;
type AMidiInputPortSend = unsafe extern "C" fn(*const AMidiInputPort, *const u8, c_ulong) -> c_long;
type AMidiInputPortClose = unsafe extern "C" fn(*const AMidiInputPort);
type AMidiOutputPortReceive = unsafe extern "C" fn(
    *const AMidiOutputPort,
    *mut i32,
    *mut u8,
    c_ulong,
    *mut c_ulong,
    *mut i64,
) -> c_long;

#[link(name = "dl")]
extern "C" {
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

struct AMidiSymbols {
    _handle: *mut c_void,
    device_from_java: AMidiDeviceFromJava,
    device_release: AMidiDeviceRelease,
    device_get_num_input_ports: AMidiDeviceGetNumPorts,
    device_get_num_output_ports: AMidiDeviceGetNumPorts,
    output_port_open: AMidiOutputPortOpen,
    output_port_close: AMidiOutputPortClose,
    input_port_open: AMidiInputPortOpen,
    input_port_send: AMidiInputPortSend,
    input_port_close: AMidiInputPortClose,
    output_port_receive: AMidiOutputPortReceive,
}

unsafe impl Send for AMidiSymbols {}
unsafe impl Sync for AMidiSymbols {}

static AMIDI: OnceLock<Option<AMidiSymbols>> = OnceLock::new();

macro_rules! load_symbol {
    ($handle:expr, $name:literal, $ty:ty) => {{
        let ptr = unsafe { dlsym($handle, concat!($name, "\0").as_ptr() as *const c_char) };
        if ptr.is_null() {
            return None;
        }
        unsafe { std::mem::transmute::<*mut c_void, $ty>(ptr) }
    }};
}

fn amidi_symbols() -> Option<&'static AMidiSymbols> {
    AMIDI.get_or_init(load_amidi_symbols).as_ref()
}

fn load_amidi_symbols() -> Option<AMidiSymbols> {
    let handle = unsafe {
        dlopen(
            b"libamidi.so\0".as_ptr() as *const c_char,
            RTLD_NOW | RTLD_LOCAL,
        )
    };
    if handle.is_null() {
        return None;
    }
    Some(AMidiSymbols {
        _handle: handle,
        device_from_java: load_symbol!(handle, "AMidiDevice_fromJava", AMidiDeviceFromJava),
        device_release: load_symbol!(handle, "AMidiDevice_release", AMidiDeviceRelease),
        device_get_num_input_ports: load_symbol!(
            handle,
            "AMidiDevice_getNumInputPorts",
            AMidiDeviceGetNumPorts
        ),
        device_get_num_output_ports: load_symbol!(
            handle,
            "AMidiDevice_getNumOutputPorts",
            AMidiDeviceGetNumPorts
        ),
        output_port_open: load_symbol!(handle, "AMidiOutputPort_open", AMidiOutputPortOpen),
        output_port_close: load_symbol!(handle, "AMidiOutputPort_close", AMidiOutputPortClose),
        input_port_open: load_symbol!(handle, "AMidiInputPort_open", AMidiInputPortOpen),
        input_port_send: load_symbol!(handle, "AMidiInputPort_send", AMidiInputPortSend),
        input_port_close: load_symbol!(handle, "AMidiInputPort_close", AMidiInputPortClose),
        output_port_receive: load_symbol!(
            handle,
            "AMidiOutputPort_receive",
            AMidiOutputPortReceive
        ),
    })
}

pub unsafe fn AMidiDevice_fromJava(
    env: *mut JNIEnv,
    midiDeviceObj: jobject,
    outDevicePtrPtr: *mut *mut AMidiDevice,
) -> media_status_t {
    if let Some(symbols) = amidi_symbols() {
        return unsafe { (symbols.device_from_java)(env, midiDeviceObj, outDevicePtrPtr) };
    }
    if !outDevicePtrPtr.is_null() {
        unsafe { *outDevicePtrPtr = ptr::null_mut() };
    }
    AMEDIA_ERROR_UNKNOWN
}

pub unsafe fn AMidiDevice_release(midiDevice: *const AMidiDevice) -> media_status_t {
    if let Some(symbols) = amidi_symbols() {
        return unsafe { (symbols.device_release)(midiDevice) };
    }
    AMEDIA_ERROR_UNKNOWN
}

pub unsafe fn AMidiDevice_getNumInputPorts(device: *const AMidiDevice) -> c_long {
    if let Some(symbols) = amidi_symbols() {
        return unsafe { (symbols.device_get_num_input_ports)(device) };
    }
    0
}

pub unsafe fn AMidiDevice_getNumOutputPorts(device: *const AMidiDevice) -> c_long {
    if let Some(symbols) = amidi_symbols() {
        return unsafe { (symbols.device_get_num_output_ports)(device) };
    }
    0
}

pub unsafe fn AMidiOutputPort_open(
    device: *const AMidiDevice,
    portNumber: i32,
    outOutputPortPtr: *mut *mut AMidiOutputPort,
) -> media_status_t {
    if let Some(symbols) = amidi_symbols() {
        return unsafe { (symbols.output_port_open)(device, portNumber, outOutputPortPtr) };
    }
    if !outOutputPortPtr.is_null() {
        unsafe { *outOutputPortPtr = ptr::null_mut() };
    }
    AMEDIA_ERROR_UNKNOWN
}

pub unsafe fn AMidiOutputPort_close(outputPort: *const AMidiOutputPort) {
    if let Some(symbols) = amidi_symbols() {
        unsafe { (symbols.output_port_close)(outputPort) };
    }
}

pub unsafe fn AMidiInputPort_open(
    device: *const AMidiDevice,
    portNumber: i32,
    outInputPortPtr: *mut *mut AMidiInputPort,
) -> media_status_t {
    if let Some(symbols) = amidi_symbols() {
        return unsafe { (symbols.input_port_open)(device, portNumber, outInputPortPtr) };
    }
    if !outInputPortPtr.is_null() {
        unsafe { *outInputPortPtr = ptr::null_mut() };
    }
    AMEDIA_ERROR_UNKNOWN
}

pub unsafe fn AMidiInputPort_send(
    inputPort: *const AMidiInputPort,
    buffer: *const u8,
    numBytes: c_ulong,
) -> c_long {
    if let Some(symbols) = amidi_symbols() {
        return unsafe { (symbols.input_port_send)(inputPort, buffer, numBytes) };
    }
    -1
}

pub unsafe fn AMidiInputPort_close(inputPort: *const AMidiInputPort) {
    if let Some(symbols) = amidi_symbols() {
        unsafe { (symbols.input_port_close)(inputPort) };
    }
}

pub unsafe fn AMidiOutputPort_receive(
    outputPort: *const AMidiOutputPort,
    opcodePtr: *mut i32,
    buffer: *mut u8,
    maxBytes: c_ulong,
    numBytesReceivedPtr: *mut c_ulong,
    outTimestampPtr: *mut i64,
) -> c_long {
    if let Some(symbols) = amidi_symbols() {
        return unsafe {
            (symbols.output_port_receive)(
                outputPort,
                opcodePtr,
                buffer,
                maxBytes,
                numBytesReceivedPtr,
                outTimestampPtr,
            )
        };
    }
    if !numBytesReceivedPtr.is_null() {
        unsafe { *numBytesReceivedPtr = 0 };
    }
    -1
}
