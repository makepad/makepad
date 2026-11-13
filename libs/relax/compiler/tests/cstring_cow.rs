// alloc/src/ffi/c_str.rs: `From<CString>`, `From<&CStr>` and `From<&CString>` for `Cow<CStr>`, and
// `From<Cow<CStr>>` for `CString`. objc2 names a defined class's ivars with `Cow<'static, CStr>`
// values it builds with `CString::new(format!(..)).unwrap().into()`.
use std::borrow::Cow;
use std::ffi::{CStr, CString};

#[inline(never)]
fn names(class: &str) -> (Cow<'static, CStr>, Cow<'static, CStr>) {
    (CString::new(format!("{class}_ivars")).unwrap().into(), CString::new(format!("{class}_drop_flag")).unwrap().into())
}

#[test]
fn c_strings_convert_into_cow_and_back() {
    let (ivars, flag) = names("Delegate");
    assert!(matches!(ivars, Cow::Owned(_)));
    assert_eq!(ivars.to_bytes(), b"Delegate_ivars");
    assert_eq!(flag.to_bytes_with_nul(), b"Delegate_drop_flag\0");

    let fixed: &'static CStr = CStr::from_bytes_with_nul(b"ivars\0").unwrap();
    let borrowed: Cow<'static, CStr> = fixed.into();
    assert!(matches!(borrowed, Cow::Borrowed(_)));
    assert_eq!(borrowed.to_bytes(), b"ivars");

    let owned = CString::new("by ref").unwrap();
    let by_ref: Cow<'_, CStr> = (&owned).into();
    assert!(matches!(by_ref, Cow::Borrowed(_)));
    assert_eq!(&*by_ref, owned.as_c_str());

    let back: CString = ivars.into();
    assert_eq!(back.as_bytes(), b"Delegate_ivars");
    let copied: CString = borrowed.into();
    assert_eq!(copied.as_bytes(), b"ivars");
}
