// A macro imported under another name: `use std::{eprintln as log_error};` then
// `crate::log_error!(..)` (rustc: macros are importable items, the use's #[cfg] applies).
// wayland-backend's logging shim (libs/linux/wayland-backend/src/lib.rs:72-77) -- before:
// "macro `log_error!` is not supported" (counter on Linux/Wayland, node 165).
#[allow(unused_imports)]
use std::{eprintln as log_error, format as fmt_alias};
#[cfg(feature = "never_on")]
use std::{panic as log_error2};
#[cfg(not(feature = "never_on"))]
use std::{format as log_error2};
#[test] fn t() {
    crate::log_error!("alias of eprintln {}", 1);
    let s = fmt_alias!("{}-{}", 1, 2);
    assert_eq!(s, "1-2");
    assert_eq!(crate::log_error2!("{}", 7), "7");
}
