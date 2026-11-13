// `Default for &CStr` (core ffi/c_str.rs, stable 1.10): the empty C string. A plug-in id read as
// `descriptor.id().unwrap_or_default()` (clack-plugin 0.2's factory) was E0277 here.
use std::ffi::CStr;

struct Descriptor {
    id: Option<&'static CStr>,
}

impl Descriptor {
    fn id(&self) -> Option<&CStr> {
        self.id
    }
}

#[test]
fn a01_empty() {
    let empty = <&CStr>::default();
    println!("| {:?} {} {:?} {}", empty, empty.is_empty(), empty.to_bytes_with_nul(), empty == c"");
}

#[test]
fn a02_unwrap_or_default() {
    let named = Descriptor { id: Some(c"org.relax.gain") };
    let unnamed = Descriptor { id: None };
    println!("| {:?} {:?}", named.id().unwrap_or_default(), unnamed.id().unwrap_or_default());
    println!("| {}", unnamed.id().unwrap_or_default() == named.id().unwrap_or_default());
}

#[derive(Default, Debug)]
struct Names<'a> {
    first: &'a CStr,
    rest: Option<&'a CStr>,
}

#[test]
fn a03_derived() {
    let n = Names::default();
    println!("| {:?} {:?} {:?}", n, n.first, n.rest);
}
