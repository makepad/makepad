// Stable core/alloc/std APIs larger crates call: NonZero conversions and checked/saturating
// arithmetic, CString::from_vec_with_nul and its error, downcasting a boxed `Error + Send`,
// `future::Ready::into_inner`, the `std::string::Drain`/`FromUtf16Error` and `std::ffi` error
// paths, and `ptr::copy`.
// Run with `makepad test` and rustc --test; lines "| " are compared.
use std::error::Error;
use std::ffi::{CString, FromVecWithNulError, NulError};
use std::num::NonZero;

#[test]
fn nonzero_conversions_and_arithmetic() {
    let a = NonZero::new(300u32).unwrap();
    let narrow: Result<NonZero<u8>, _> = NonZero::<u8>::try_from(a);
    let fits = NonZero::<u16>::try_from(a).map(|v| v.get());
    let signed = NonZero::<i64>::try_from(NonZero::new(5u64).unwrap()).map(|v| v.get());
    let b = NonZero::new(70_000u32).unwrap();
    println!(
        "| {} {:?} {:?} {:?} {} {:?} {}",
        narrow.is_err(),
        fits,
        signed,
        a.checked_mul(b).map(|v| v.get()),
        a.saturating_mul(b).get(),
        NonZero::new(3u8).unwrap().checked_pow(6).map(|v| v.get()),
        NonZero::new(3u8).unwrap().saturating_pow(6).get(),
    );
}

#[test]
fn cstring_from_vec_with_nul() {
    let ok = CString::from_vec_with_nul(b"abc\0".to_vec()).unwrap();
    let missing: FromVecWithNulError = CString::from_vec_with_nul(b"abc".to_vec()).unwrap_err();
    let inner: FromVecWithNulError = CString::from_vec_with_nul(b"a\0b\0".to_vec()).unwrap_err();
    let nul: NulError = CString::new(b"x\0y".to_vec()).unwrap_err();
    let bytes = inner.as_bytes().to_vec();
    println!("| {:?} {} {:?} {} {}", ok, missing, bytes, inner.into_bytes().len(), nul.nul_position());
}

#[derive(Debug)]
struct Mine(u8);
impl std::fmt::Display for Mine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "mine {}", self.0)
    }
}
impl Error for Mine {}

#[test]
fn boxed_send_errors_downcast() {
    let e: Box<dyn Error + Send> = Box::new(Mine(4));
    let back = e.downcast::<Mine>().map(|m| m.0);
    let e: Box<dyn Error + Send> = Box::new(Mine(5));
    let wrong = e.downcast::<std::fmt::Error>().map_err(|e| e.to_string());
    let e: Box<dyn Error + Send + Sync> = Box::new(Mine(6));
    println!("| {:?} {:?} {:?}", back, wrong, e.downcast::<Mine>().map(|m| m.0));
}

#[test]
fn ready_into_inner_and_string_paths() {
    let r = std::future::ready(String::from("ready"));
    let mut s = String::from("hello world");
    let d: std::string::Drain<'_> = s.drain(..6);
    let drained: String = d.collect();
    let bad: Result<String, std::string::FromUtf16Error> = String::from_utf16(&[0xD800]);
    let mut buf = [0u8; 4];
    unsafe { std::ptr::copy([1u8, 2, 3].as_ptr(), buf.as_mut_ptr().add(1), 3) };
    println!("| {} {:?} {} {} {:?}", r.into_inner(), drained, s, bad.is_err(), buf);
}
