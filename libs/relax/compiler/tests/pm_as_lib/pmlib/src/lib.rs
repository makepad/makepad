// a plain library (not a proc-macro crate) that links the sysroot's proc_macro, as proc-macro2 does:
// outside a macro call is_available() is false and the caller takes its own fallback
extern crate proc_macro;

pub fn avail() -> bool {
    proc_macro::is_available()
}

pub fn tokens(s: &str) -> String {
    if proc_macro::is_available() { s.parse::<proc_macro::TokenStream>().unwrap().to_string() } else { format!("fallback({s})") }
}
