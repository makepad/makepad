//! Android MIDI (AMidi) FFI bindings.
//!
//! `libamidi.so` was added in Android 10 / API 29. To allow Makepad apps to
//! target a lower `minSdkVersion` (e.g. API 26 for broader device coverage),
//! we **don't** statically link against `libamidi.so`. Instead, the symbols
//! are resolved at runtime via `dlopen(libamidi.so)` + `dlsym(...)`. On
//! devices below API 29 the library isn't present and every entry point
//! below returns an error, gracefully disabling MIDI; on API 29+ devices
//! they call through to the OS implementation.
//!
//! Callers don't need to know about this — the public `pub unsafe fn`
//! signatures match the original `extern "C"` block, and a missing symbol
//! is reported as `media_status_t = -1`.
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
use crate::os::linux::module_loader::ModuleLoader;
use jni_sys::*;
use makepad_jni_sys as jni_sys;
use std::os::raw::{c_long, c_ulong};
use std::sync::OnceLock;

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
/// Error returned when libamidi.so is not present (i.e. device API < 29).
const AMIDI_UNAVAILABLE: media_status_t = -1;

type Fn_AMidiDevice_fromJava =
    unsafe extern "C" fn(*mut JNIEnv, jobject, *mut *mut AMidiDevice) -> media_status_t;
type Fn_AMidiDevice_release = unsafe extern "C" fn(*const AMidiDevice) -> media_status_t;
type Fn_AMidiDevice_getNumInputPorts = unsafe extern "C" fn(*const AMidiDevice) -> c_long;
type Fn_AMidiDevice_getNumOutputPorts = unsafe extern "C" fn(*const AMidiDevice) -> c_long;
type Fn_AMidiOutputPort_open =
    unsafe extern "C" fn(*const AMidiDevice, i32, *mut *mut AMidiOutputPort) -> media_status_t;
type Fn_AMidiOutputPort_close = unsafe extern "C" fn(*const AMidiOutputPort);
type Fn_AMidiInputPort_open =
    unsafe extern "C" fn(*const AMidiDevice, i32, *mut *mut AMidiInputPort) -> media_status_t;
type Fn_AMidiInputPort_send =
    unsafe extern "C" fn(*const AMidiInputPort, *const u8, c_ulong) -> c_long;
type Fn_AMidiInputPort_close = unsafe extern "C" fn(*const AMidiInputPort);
type Fn_AMidiOutputPort_receive = unsafe extern "C" fn(
    *const AMidiOutputPort,
    *mut i32,
    *mut u8,
    c_ulong,
    *mut c_ulong,
    *mut i64,
) -> c_long;

struct AMidiVtable {
    device_from_java: Fn_AMidiDevice_fromJava,
    device_release: Fn_AMidiDevice_release,
    device_get_num_input_ports: Fn_AMidiDevice_getNumInputPorts,
    device_get_num_output_ports: Fn_AMidiDevice_getNumOutputPorts,
    output_port_open: Fn_AMidiOutputPort_open,
    output_port_close: Fn_AMidiOutputPort_close,
    input_port_open: Fn_AMidiInputPort_open,
    input_port_send: Fn_AMidiInputPort_send,
    input_port_close: Fn_AMidiInputPort_close,
    output_port_receive: Fn_AMidiOutputPort_receive,
}

// All fields are raw fn pointers, which are Send+Sync.
unsafe impl Send for AMidiVtable {}
unsafe impl Sync for AMidiVtable {}

/// Cached vtable for `libamidi.so`. `None` means the library couldn't be loaded
/// (typically because we're on API < 29 and the OS doesn't provide it).
static AMIDI: OnceLock<Option<AMidiVtable>> = OnceLock::new();

fn vtable() -> Option<&'static AMidiVtable> {
    AMIDI
        .get_or_init(|| {
            // Try to load libamidi.so. If it isn't available we cache the None and
            // every entry point becomes a graceful no-op for the rest of the
            // process lifetime.
            let lib = ModuleLoader::load("libamidi.so").ok()?;
            let vt = AMidiVtable {
                device_from_java: lib.get_symbol("AMidiDevice_fromJava").ok()?,
                device_release: lib.get_symbol("AMidiDevice_release").ok()?,
                device_get_num_input_ports: lib.get_symbol("AMidiDevice_getNumInputPorts").ok()?,
                device_get_num_output_ports: lib
                    .get_symbol("AMidiDevice_getNumOutputPorts")
                    .ok()?,
                output_port_open: lib.get_symbol("AMidiOutputPort_open").ok()?,
                output_port_close: lib.get_symbol("AMidiOutputPort_close").ok()?,
                input_port_open: lib.get_symbol("AMidiInputPort_open").ok()?,
                input_port_send: lib.get_symbol("AMidiInputPort_send").ok()?,
                input_port_close: lib.get_symbol("AMidiInputPort_close").ok()?,
                output_port_receive: lib.get_symbol("AMidiOutputPort_receive").ok()?,
            };
            // Leak the ModuleLoader so libamidi.so stays mapped for the symbol
            // pointers we just captured. dlclose() in Drop would only decrement
            // the refcount, but this avoids relying on libamidi being kept alive
            // by anyone else — and we want to load it exactly once anyway.
            std::mem::forget(lib);
            Some(vt)
        })
        .as_ref()
}

// Wrapper functions matching the original extern "C" signatures. Each one
// looks up the cached symbol; when the library isn't loaded (API < 29) it
// returns an error code so callers can no-op gracefully.

pub unsafe fn AMidiDevice_fromJava(
    env: *mut JNIEnv,
    midiDeviceObj: jobject,
    outDevicePtrPtr: *mut *mut AMidiDevice,
) -> media_status_t {
    match vtable() {
        Some(vt) => (vt.device_from_java)(env, midiDeviceObj, outDevicePtrPtr),
        None => AMIDI_UNAVAILABLE,
    }
}

pub unsafe fn AMidiDevice_release(midiDevice: *const AMidiDevice) -> media_status_t {
    match vtable() {
        Some(vt) => (vt.device_release)(midiDevice),
        None => AMIDI_UNAVAILABLE,
    }
}

pub unsafe fn AMidiDevice_getNumInputPorts(device: *const AMidiDevice) -> c_long {
    match vtable() {
        Some(vt) => (vt.device_get_num_input_ports)(device),
        None => 0,
    }
}

pub unsafe fn AMidiDevice_getNumOutputPorts(device: *const AMidiDevice) -> c_long {
    match vtable() {
        Some(vt) => (vt.device_get_num_output_ports)(device),
        None => 0,
    }
}

pub unsafe fn AMidiOutputPort_open(
    device: *const AMidiDevice,
    portNumber: i32,
    outOutputPortPtr: *mut *mut AMidiOutputPort,
) -> media_status_t {
    match vtable() {
        Some(vt) => (vt.output_port_open)(device, portNumber, outOutputPortPtr),
        None => AMIDI_UNAVAILABLE,
    }
}

pub unsafe fn AMidiOutputPort_close(outputPort: *const AMidiOutputPort) {
    if let Some(vt) = vtable() {
        (vt.output_port_close)(outputPort);
    }
}

pub unsafe fn AMidiInputPort_open(
    device: *const AMidiDevice,
    portNumber: i32,
    outInputPortPtr: *mut *mut AMidiInputPort,
) -> media_status_t {
    match vtable() {
        Some(vt) => (vt.input_port_open)(device, portNumber, outInputPortPtr),
        None => AMIDI_UNAVAILABLE,
    }
}

pub unsafe fn AMidiInputPort_send(
    inputPort: *const AMidiInputPort,
    buffer: *const u8,
    numBytes: c_ulong,
) -> c_long {
    match vtable() {
        Some(vt) => (vt.input_port_send)(inputPort, buffer, numBytes),
        // Return -1 to indicate failure (matches AMidi convention for the
        // "send" family).
        None => -1,
    }
}

pub unsafe fn AMidiInputPort_close(inputPort: *const AMidiInputPort) {
    if let Some(vt) = vtable() {
        (vt.input_port_close)(inputPort);
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
    match vtable() {
        Some(vt) => (vt.output_port_receive)(
            outputPort,
            opcodePtr,
            buffer,
            maxBytes,
            numBytesReceivedPtr,
            outTimestampPtr,
        ),
        // 0 received messages is a valid AMidi response.
        None => 0,
    }
}
