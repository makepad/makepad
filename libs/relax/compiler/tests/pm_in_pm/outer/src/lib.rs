//! A proc-macro crate that itself uses a derive of another proc-macro crate (the macro world needs
//! the inner crate's derive while it builds the outer crate).
use pmpm_hello::Hello;
use proc_macro::TokenStream;

#[derive(pmpm_inner::Hello)]
struct Helper;

#[derive(pmpm_inner::Hello)]
#[allow(dead_code)]
enum Other { A }

#[proc_macro]
pub fn outer(_input: TokenStream) -> TokenStream {
    format!("\"{} {}\"", <Helper as Hello>::hello(), <Other as Hello>::hello()).parse().unwrap()
}
