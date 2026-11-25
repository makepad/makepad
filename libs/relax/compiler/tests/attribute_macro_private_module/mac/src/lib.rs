use proc_macro::TokenStream;

#[proc_macro_attribute]
pub fn generate(_: TokenStream, item: TokenStream) -> TokenStream {
    let mut output = item;
    output.extend("pub struct Generated;".parse::<TokenStream>().unwrap());
    output
}

/// `#[service]`: the item, and a constant beside it.
#[proc_macro_attribute]
pub fn service(_: TokenStream, item: TokenStream) -> TokenStream {
    let mut output = item;
    output.extend("pub const SERVED: u32 = 9;".parse::<TokenStream>().unwrap());
    output
}
