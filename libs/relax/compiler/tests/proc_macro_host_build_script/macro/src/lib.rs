use proc_macro::TokenStream;

// the proc macro's own hb-gen has no `wide`: its build script's run is the host side's
#[proc_macro]
pub fn total(_: TokenStream) -> TokenStream {
    format!("{}u32", hb_gen::total()).parse().unwrap()
}
