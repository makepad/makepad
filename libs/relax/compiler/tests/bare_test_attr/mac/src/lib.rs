use proc_macro::{TokenStream, TokenTree};

#[proc_macro_attribute]
pub fn test(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut tokens = item.into_iter();
    while let Some(token) = tokens.next() {
        if matches!(&token, TokenTree::Ident(id) if id.to_string() == "fn") {
            let name = tokens.next().unwrap().to_string();
            return format!("pub fn {}() {{ println!(\"macro: {}\"); }}", name, name).parse().unwrap();
        }
    }
    panic!("expected a function")
}

#[proc_macro_attribute]
pub fn other(attr: TokenStream, item: TokenStream) -> TokenStream {
    test(attr, item)
}
