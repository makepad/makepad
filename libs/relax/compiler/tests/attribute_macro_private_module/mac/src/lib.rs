use proc_macro::TokenStream;

#[proc_macro_attribute]
pub fn generate(_: TokenStream, item: TokenStream) -> TokenStream {
    let mut output = item;
    output.extend("pub struct Generated;".parse::<TokenStream>().unwrap());
    output
}
