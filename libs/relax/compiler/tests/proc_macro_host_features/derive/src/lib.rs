use proc_macro::{TokenStream, TokenTree};

// `impl hf_trait::Rate for <the struct>`, a rate of 30
#[proc_macro_derive(Rate)]
pub fn derive_rate(input: TokenStream) -> TokenStream {
    let mut after_struct = false;
    let mut name = String::new();
    for t in input {
        if let TokenTree::Ident(i) = t {
            let s = i.to_string();
            if after_struct {
                name = s;
                break;
            }
            after_struct = s == "struct";
        }
    }
    format!("impl hf_trait::Rate for {name} {{ fn rate(&self) -> u32 {{ 30 }} }}").parse().unwrap()
}
