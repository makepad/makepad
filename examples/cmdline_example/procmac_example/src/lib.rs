#![allow(unused)]
#![feature(proc_macro_span)]
use proc_macro::{TokenStream, TokenTree};

use makepad_macro_lib::{
    TokenBuilder,
    TokenParser,
};

#[proc_macro_derive(DeriveExample)]
pub fn derive_example(input: TokenStream) -> TokenStream {
    for k in input{
        if let TokenTree::Group(g) = k{
            eprintln!("Group: {:?}",g.delimiter());
            for k in g.stream(){
                eprintln!("--- {:?}",k);
            }
        }
        else{
            eprintln!("{:?}",k);
        }
    }
    
    TokenStream::new()
}

#[proc_macro]
pub fn function_example(input: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new();
    let mut parser = TokenParser::new(input);

    let lit_a = parser.eat_literal().unwrap();
    let op = parser.eat_any_punct().unwrap();
    let lit_b = parser.eat_literal().unwrap();

    let a = lit_a.to_string().parse::<u32>().unwrap();
    let b = lit_b.to_string().parse::<u32>().unwrap();
    
    tb.suf_u32(a * b);
    
    tb.end()
}
