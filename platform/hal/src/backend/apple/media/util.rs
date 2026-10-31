//! Small helpers of the media module: Core Foundation strings and CoreAudio / Foundation error codes.
//! Moved from os/apple/apple_util.rs.

use makepad_apple_sys::*;

pub unsafe fn ccfstr_from_str(inp: &str) -> CFStringRef {
    let null = format!("{}\0", inp);
    __CFStringMakeConstantString(null.as_ptr() as *const ::std::os::raw::c_char)
}

pub unsafe fn cfstring_ref_to_string(cfstring: CFStringRef) -> String {
    let length = CFStringGetLength(cfstring);
    let range = CFRange {
        location: 0,
        length,
    };
    let mut num_bytes = 0u64;
    let converted = CFStringGetBytes(
        cfstring,
        range,
        kCFStringEncodingUTF8,
        0,
        false,
        0 as *mut u8,
        0,
        &mut num_bytes,
    );
    if converted == 0 || num_bytes == 0 {
        return String::new();
    }
    let mut buffer = Vec::new();
    buffer.resize(num_bytes as usize, 0u8);
    CFStringGetBytes(
        cfstring,
        range,
        kCFStringEncodingUTF8,
        0,
        false,
        buffer.as_mut_ptr() as *mut u8,
        num_bytes,
        0 as *mut u64,
    );
    if let Ok(val) = String::from_utf8(buffer) {
        val
    } else {
        String::new()
    }
}

#[derive(Debug)]
#[repr(i32)]
pub enum OSError {
    Unimplemented = -4,
    FileNotFound = -43,
    FilePermission = -54,
    TooManyFilesOpen = -42,

    Unspecified = -1500,
    SystemSoundClientMessageTimeout = -1501,

    BadFilePath = 561017960,
    Param = -50,
    MemFull = -108,

    FormatUnspecified = 2003329396,
    UnknownProperty = 2003332927,
    BadPropertySize = 561211770,
    IllegalOperation = 1852797029,
    UnsupportedFormat = 560226676,
    State = 561214580,
    NotEnoughBufferSpace = 560100710,

    UnsupportedDataFormat = 1718449215,

    InvalidProperty = -10879,
    InvalidParameter = -10878,
    InvalidElement = -10877,
    NoConnection = -10876,
    FailedInitialization = -10875,
    TooManyFramesToProcess = -10874,
    InvalidFile = -10871,
    FormatNotSupported = -10868,
    Uninitialized = -10867,
    InvalidScope = -10866,
    PropertyNotWritable = -10865,
    CannotDoInCurrentContext = -10863,
    InvalidPropertyValue = -10851,
    PropertyNotInUse = -10850,
    Initialized = -10849,
    InvalidOfflineRender = -10848,
    Unauthorized = -10847,

    Unknown,
}

impl OSError {
    pub fn from(result: i32) -> Result<(), Self> {
        Err(match result {
            0 => return Ok(()),
            x if x == Self::Unimplemented as i32 => Self::Unimplemented,
            x if x == Self::FileNotFound as i32 => Self::FileNotFound,
            x if x == Self::FilePermission as i32 => Self::FilePermission,
            x if x == Self::TooManyFilesOpen as i32 => Self::TooManyFilesOpen,

            x if x == Self::Unspecified as i32 => Self::Unspecified,
            x if x == Self::SystemSoundClientMessageTimeout as i32 => {
                Self::SystemSoundClientMessageTimeout
            }

            x if x == Self::BadFilePath as i32 => Self::BadFilePath,
            x if x == Self::Param as i32 => Self::Param,
            x if x == Self::MemFull as i32 => Self::MemFull,

            x if x == Self::FormatUnspecified as i32 => Self::FormatUnspecified,
            x if x == Self::UnknownProperty as i32 => Self::UnknownProperty,
            x if x == Self::BadPropertySize as i32 => Self::BadPropertySize,
            x if x == Self::IllegalOperation as i32 => Self::IllegalOperation,
            x if x == Self::UnsupportedFormat as i32 => Self::UnsupportedFormat,
            x if x == Self::State as i32 => Self::State,
            x if x == Self::NotEnoughBufferSpace as i32 => Self::NotEnoughBufferSpace,

            x if x == Self::UnsupportedDataFormat as i32 => Self::UnsupportedDataFormat,

            x if x == Self::InvalidProperty as i32 => Self::InvalidProperty,
            x if x == Self::InvalidParameter as i32 => Self::InvalidParameter,
            x if x == Self::InvalidElement as i32 => Self::InvalidElement,
            x if x == Self::NoConnection as i32 => Self::NoConnection,
            x if x == Self::FailedInitialization as i32 => Self::FailedInitialization,
            x if x == Self::TooManyFramesToProcess as i32 => Self::TooManyFramesToProcess,
            x if x == Self::InvalidFile as i32 => Self::InvalidFile,
            x if x == Self::FormatNotSupported as i32 => Self::FormatNotSupported,
            x if x == Self::Uninitialized as i32 => Self::Uninitialized,
            x if x == Self::InvalidScope as i32 => Self::InvalidScope,
            x if x == Self::PropertyNotWritable as i32 => Self::PropertyNotWritable,
            x if x == Self::CannotDoInCurrentContext as i32 => Self::CannotDoInCurrentContext,
            x if x == Self::InvalidPropertyValue as i32 => Self::InvalidPropertyValue,
            x if x == Self::PropertyNotInUse as i32 => Self::PropertyNotInUse,
            x if x == Self::Initialized as i32 => Self::Initialized,
            x if x == Self::InvalidOfflineRender as i32 => Self::InvalidOfflineRender,
            x if x == Self::Unauthorized as i32 => Self::Unauthorized,
            _ => Self::Unknown,
        })
    }

    pub fn from_nserror(ns_error: ObjcId) -> Result<(), Self> {
        if ns_error != nil {
            let code: i32 = unsafe { msg_send![ns_error, code] };
            Self::from(code)
        } else {
            Ok(())
        }
    }
}

