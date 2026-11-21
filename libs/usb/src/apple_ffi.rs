//! Hand-declared macOS SDK ABI: IOKitLib.h, CoreFoundation, objc, and IOUSBHost.
use std::ffi::{c_char, c_void};

pub type Object = *mut c_void;
pub type Sel = *mut c_void;
pub type Cf = *const c_void;
pub type Io = u32;
pub type IOReturn = i32;
// objc.h uses signed char on Intel macOS and C bool on Apple Silicon.
#[cfg(target_arch = "aarch64")]
pub type Bool = bool;
#[cfg(not(target_arch = "aarch64"))]
pub type Bool = i8;

#[repr(C, packed)]
pub struct DeviceRequest {
    pub request_type: u8,
    pub request: u8,
    pub value: u16,
    pub index: u16,
    pub length: u16,
}

#[repr(C)]
pub struct BlockDescriptor {
    pub reserved: usize,
    pub size: usize,
}

const _: () = {
    assert!(std::mem::size_of::<DeviceRequest>() == 8);
    assert!(std::mem::align_of::<DeviceRequest>() == 1);
};

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    pub fn IOServiceMatching(name: *const c_char) -> *mut c_void;
    pub fn IORegistryEntryIDMatching(id: u64) -> *mut c_void;
    pub fn IOServiceGetMatchingServices(port: u32, matching: Cf, iterator: *mut Io) -> IOReturn;
    pub fn IOServiceGetMatchingService(port: u32, matching: Cf) -> Io;
    pub fn IOIteratorNext(iterator: Io) -> Io;
    pub fn IOObjectRelease(object: Io) -> IOReturn;
    pub fn IOObjectGetClass(object: Io, name: *mut c_char) -> IOReturn;
    pub fn IOObjectConformsTo(object: Io, name: *const c_char) -> u32;
    pub fn IORegistryEntryGetRegistryEntryID(entry: Io, id: *mut u64) -> IOReturn;
    pub fn IORegistryEntryGetChildIterator(entry: Io, plane: *const c_char, iterator: *mut Io) -> IOReturn;
    pub fn IORegistryEntryCreateCFProperty(entry: Io, key: Cf, allocator: Cf, options: u32) -> Cf;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    pub fn CFStringCreateWithCString(allocator: Cf, text: *const c_char, encoding: u32) -> Cf;
    pub fn CFGetTypeID(value: Cf) -> usize;
    pub fn CFNumberGetTypeID() -> usize;
    pub fn CFNumberGetValue(value: Cf, kind: isize, out: *mut c_void) -> u8;
    pub fn CFStringGetTypeID() -> usize;
    pub fn CFStringGetLength(value: Cf) -> isize;
    pub fn CFStringGetMaximumSizeForEncoding(length: isize, encoding: u32) -> isize;
    pub fn CFStringGetCString(value: Cf, buffer: *mut c_char, size: isize, encoding: u32) -> u8;
    pub fn CFRelease(value: Cf);
}

#[link(name = "Foundation", kind = "framework")]
#[link(name = "IOUSBHost", kind = "framework")]
extern "C" {}

#[link(name = "objc")]
extern "C" {
    pub fn objc_getClass(name: *const c_char) -> Object;
    pub fn sel_registerName(name: *const c_char) -> Sel;
    pub fn objc_msgSend();
    pub fn objc_autoreleasePoolPush() -> *mut c_void;
    pub fn objc_autoreleasePoolPop(pool: *mut c_void);
}

#[link(name = "System")]
extern "C" {
    pub static _NSConcreteStackBlock: [*const c_void; 32];
}

// Each use supplies the exact Objective-C signature and must be in a documented
// unsafe block. No variadic calls: notably, NSTimeInterval is double, not an integer.
macro_rules! message {
    ($signature:ty, $object:expr, $selector:expr $(, $argument:expr)* $(,)?) => {{
        let send: $signature = std::mem::transmute($crate::apple_ffi::objc_msgSend as unsafe extern "C" fn());
        send($object, $crate::apple_ffi::sel_registerName($selector.as_ptr()) $(, $argument)*)
    }};
}
pub(crate) use message;

pub const UTF8: u32 = 0x0800_0100;
pub const SINT64: isize = 4;
pub const INIT_NONE: usize = 0; // IOUSBHostObjectInitOptionsNone
pub const ABORT_SYNCHRONOUS: usize = 1; // IOUSBHostAbortOptionSynchronous
pub const ERROR: IOReturn = 0xe000_02bc_u32 as i32;
pub const NO_DEVICE: IOReturn = 0xe000_02c0_u32 as i32;
pub const NOT_PRIVILEGED: IOReturn = 0xe000_02c1_u32 as i32;
pub const EXCLUSIVE_ACCESS: IOReturn = 0xe000_02c5_u32 as i32;
pub const BUSY: IOReturn = 0xe000_02d5_u32 as i32;
pub const TIMEOUT: IOReturn = 0xe000_02d6_u32 as i32;
pub const ABORTED: IOReturn = 0xe000_02eb_u32 as i32;
pub const NOT_RESPONDING: IOReturn = 0xe000_02ed_u32 as i32;
pub const PIPE_STALLED: IOReturn = 0xe000_404f_u32 as i32;
pub const TRANSACTION_TIMEOUT: IOReturn = 0xe000_4051_u32 as i32;
