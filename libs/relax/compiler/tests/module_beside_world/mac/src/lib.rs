// `#[derive(Answer)]` gives the type `fn answer() -> u32`, summed from four library crates: the
// macro world holds more crates than the program has before its modules load.
use proc_macro::{TokenStream, TokenTree};

#[proc_macro_derive(Answer)]
pub fn answer(input: TokenStream) -> TokenStream {
    let mut name = None;
    let mut after_struct = false;
    for t in input {
        if let TokenTree::Ident(i) = t {
            if after_struct {
                name = Some(i.to_string());
                break;
            }
            after_struct = i.to_string() == "struct";
        }
    }
    let sum = mbw_l1::part() + mbw_l2::part() + mbw_l3::part() + mbw_l4::part() + 22;
    format!("impl {} {{ pub fn answer() -> u32 {{ {} }} }}", name.expect("a struct"), sum).parse().unwrap()
}
