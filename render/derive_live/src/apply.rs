use proc_macro::TokenStream;
use crate::macro_lib::*;

pub fn nodes_impl(input:TokenStream)->TokenStream{
    let mut parser = TokenParser::new(input);
    let mut tb = TokenBuilder::new();
    // OK SO.
    // we have to parse an objectstructure
    // and possibly an array
    
    tb.end()
}