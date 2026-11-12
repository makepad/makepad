//! `Cow<CStr>` converts from `&CStr`, `CString` and `&CString`, as std's `From` impls (rustix
//! 1.1.4's `linux_raw/net/addr.rs` writes `CStr::from_bytes_with_nul_unchecked(bytes).into()`).
use std::borrow::Cow;
use std::ffi::{CStr, CString};

#[test]
fn cow_cstr_from_borrowed_and_owned() {
    let borrowed: Cow<'_, CStr> = c"abc".into();
    assert!(matches!(borrowed, Cow::Borrowed(_)), "&CStr into Cow is Borrowed");
    assert_eq!(borrowed.to_bytes(), b"abc", "&CStr into Cow keeps the bytes");

    let owned: Cow<'_, CStr> = CString::new("def").unwrap().into();
    assert!(matches!(owned, Cow::Owned(_)), "CString into Cow is Owned");
    assert_eq!(owned.to_bytes(), b"def", "CString into Cow keeps the bytes");

    let s = CString::new("ghi").unwrap();
    let by_ref: Cow<'_, CStr> = (&s).into();
    assert!(matches!(by_ref, Cow::Borrowed(_)), "&CString into Cow is Borrowed");
    assert_eq!(by_ref.to_bytes(), b"ghi", "&CString into Cow keeps the bytes");
}
