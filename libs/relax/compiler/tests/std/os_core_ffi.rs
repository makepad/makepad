// std-os lane: core::ffi (CStr, c types) and alloc::ffi::CString (differential: agree.sh)
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
    let c: &core::ffi::c_str::CStr = std::ffi::c_str::CStr::from_bytes_with_nul(b"box\0").unwrap();
    let boxed: Box<core::ffi::c_str::CStr> = c.into();
    let original = boxed.as_ptr();
    let owned: alloc::ffi::c_str::CString = boxed.into_c_string();
    println!("| into_c_string {:?} {}", owned, original == owned.as_ptr());
    let until: core::ffi::c_str::FromBytesUntilNulError = CStr::from_bytes_until_nul(b"x").unwrap_err();
    let with: core::ffi::c_str::FromBytesWithNulError = CStr::from_bytes_with_nul(b"x").unwrap_err();
    let until: std::ffi::c_str::FromBytesUntilNulError = until;
    let with: std::ffi::c_str::FromBytesWithNulError = with;
    println!("| borrowed errors {} {}", until, with);
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
    let nul: alloc::ffi::c_str::NulError = std::ffi::c_str::CString::new(&b"a\0b"[..]).unwrap_err();
    let vec_nul: alloc::ffi::c_str::FromVecWithNulError = CString::from_vec_with_nul(vec![1]).unwrap_err();
    let utf8: alloc::ffi::c_str::IntoStringError = s2.into_string().unwrap_err();
    let nul: std::ffi::c_str::NulError = nul;
    let vec_nul: std::ffi::c_str::FromVecWithNulError = vec_nul;
    let utf8: std::ffi::c_str::IntoStringError = utf8;
    println!("| owned errors {} {:?} {}", nul.nul_position(), vec_nul.as_bytes(), utf8.utf8_error().valid_up_to());

    use std::ffi::os_str::{Display, OsStr, OsString};
    let mut os = OsString::from("aÉZ");
    println!("| ascii {} {}", OsStr::is_ascii(&os), OsStr::new("AZ").is_ascii());
    OsStr::make_ascii_lowercase(&mut os);
    let display: Display<'_> = os.display();
    println!("| lowercase {}", display);
    OsStr::make_ascii_uppercase(&mut os);
    println!("| uppercase {}", os.display());
}
