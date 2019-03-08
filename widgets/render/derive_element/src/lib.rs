extern crate proc_macro;
extern crate proc_macro2;
use quote::quote;
use proc_macro2::Span;
use syn::{Item, Ident};
// The actual macro
#[proc_macro_derive(Element)]
pub fn elements_derive_macro(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let parsed = syn::parse_macro_input!(input as syn::Item);

    if let Item::Struct(strct) = parsed{
        
        let ident = strct.ident;
        // lets convert CamelCase to camel_case
        let mut handle = "handle".to_string();
        for chr in ident.to_string().chars(){
            if chr.is_uppercase(){
                handle.push_str("_");
            };
            for lc in chr.to_lowercase(){
                handle.push(lc);
            }
        };
        let handle_ident = Ident::new(&handle, Span::call_site());
        let ts = proc_macro::TokenStream::from(quote!{
            impl ElementLife for #ident{
                fn construct(&mut self, cx: &mut Cx){
                    self.#handle_ident(cx, &mut Event::Construct);
                }

                fn destruct(&mut self, cx: &mut Cx){
                    self.#handle_ident(cx, &mut Event::Destruct);
                }
            }
        });
        return ts;
    };
    return proc_macro::TokenStream::new();
}