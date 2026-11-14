// a plain library (not a proc-macro crate) that links the sysroot's proc_macro, as proc-macro2 does:
// outside a macro call is_available() is false and the caller takes its own fallback
extern crate proc_macro;

pub fn avail() -> bool {
    proc_macro::is_available()
}

pub fn tokens(s: &str) -> String {
    if proc_macro::is_available() { s.parse::<proc_macro::TokenStream>().unwrap().to_string() } else { format!("fallback({s})") }
}

// proc-macro2's wrapper.rs: the compiler's C string literal (1.79) while a macro runs, its own otherwise
pub fn c_literal(s: &std::ffi::CStr) -> String {
    if proc_macro::is_available() { proc_macro::Literal::c_string(s).to_string() } else { format!("fallback({s:?})") }
}
