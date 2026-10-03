// std-os lane: core::ffi (CStr, c types) and alloc::ffi::CString (differential: os_diff.sh)
extern crate alloc;
use alloc::ffi::CString;
use core::ffi::{c_char, CStr};

#[test]
fn a01_cstr() {
    let bytes = b"hello\0";
    let c = CStr::from_bytes_with_nul(bytes).unwrap();
    println!("| {:?} {} {} {:?}", c, c.count_bytes(), c.is_empty(), c.to_str());
    println!("| {:?}", CStr::from_bytes_with_nul(b"he\0llo\0"));
    println!("| {:?}", CStr::from_bytes_with_nul(b"hello"));
    println!("| {:?}", CStr::from_bytes_until_nul(b"ab\0cd\0"));
    println!("| {:?}", CStr::from_bytes_until_nul(b"abcd"));
    match CStr::from_bytes_with_nul(b"x\0y\0") {
        Ok(_) => {}
        Err(e) => println!("| {}", e),
    }
    let weird = CStr::from_bytes_with_nul(b"t\ta\"b'\\\x01\x7f\xff\xc3\xa9\n\0").unwrap();
    println!("| {:?} {:?}", weird, weird.to_string_lossy());
    let p = c.as_ptr();
    let back = unsafe { CStr::from_ptr(p) };
    println!("| {:?} {:?} {:?}", back.to_bytes(), back.to_bytes_with_nul(), back == c);
    let e = CStr::from_bytes_with_nul(b"\0").unwrap();
    println!("| {} {:?}", e.is_empty(), e);
    let _x: c_char = 65;
}

#[test]
fn a02_cstring() {
    let s = CString::new("abc").unwrap();
    println!("| {:?} {:?} {:?}", s, s.as_bytes(), s.as_bytes_with_nul());
    let err = CString::new(vec![1u8, 0, 2]).unwrap_err();
    println!("| {:?} {} {} {:?}", err, err, err.nul_position(), err.clone().into_vec());
    let s2 = CString::new(vec![0xffu8, 0x41]).unwrap();
    println!("| {:?}", s2.clone().into_string().map_err(|e| e.utf8_error().valid_up_to()));
    println!("| {:?} {:?}", s.clone().into_string(), s.clone().into_bytes_with_nul());
    let raw = s.clone().into_raw();
    let s3 = unsafe { CString::from_raw(raw) };
    println!("| {:?} {} {:?}", s3, s3 == s, CString::default());
    let owned: CString = s3.as_c_str().to_owned();
    println!("| {:?} {}", owned, owned.to_str().unwrap());
}
