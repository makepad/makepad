use hf_shared::Rate;
use proc_macro::TokenStream;

// the proc macro's own hf-shared has `rate`: Meter implements Rate there
#[proc_macro]
pub fn measured(_: TokenStream) -> TokenStream {
    format!("{}u32", hf_shared::Meter.rate() + hf_shared::base()).parse().unwrap()
}
