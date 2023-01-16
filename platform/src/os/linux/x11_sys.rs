#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _XDisplay {
    _unused: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _XIM {
    _unused: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _XrmHashBucketRec {
    _unused: [u8; 0],
}

pub type XIM = *mut _XIM;
pub type Atom = ::std::os::raw::c_ulong;

extern "C" {
    pub fn XOpenDisplay(arg1: *const ::std::os::raw::c_char) -> *mut Display;
    pub fn XConnectionNumber(arg1: *mut Display) -> ::std::os::raw::c_int;
    pub fn XOpenIM(
        arg1: *mut Display,
        arg2: *mut _XrmHashBucketRec,
        arg3: *mut ::std::os::raw::c_char,
        arg4: *mut ::std::os::raw::c_char,
    ) -> XIM;
}
