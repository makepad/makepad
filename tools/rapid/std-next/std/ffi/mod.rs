//! std::ffi: C types and strings (from core/alloc) plus OsStr/OsString (unix: bytes).

pub use alloc::ffi::{CString, IntoStringError, NulError};
pub use core::ffi::{c_char, c_double, c_float, c_int, c_long, c_longlong, c_schar, c_short, c_uchar, c_uint, c_ulong, c_ulonglong, c_ushort, c_void};
pub use core::ffi::{CStr, FromBytesUntilNulError, FromBytesWithNulError};

mod os_str;

pub use self::os_str::{Display, OsStr, OsString};
