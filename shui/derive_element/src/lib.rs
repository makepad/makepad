extern crate proc_macro;
use quote::quote;
use syn::Item;
// The actual macro
#[proc_macro_derive(Element)]
pub fn elements_derive_macro(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let parsed = syn::parse_macro_input!(input as syn::Item);

    if let Item::Struct(strct) = parsed{
        
        let ident = strct.ident;
        let ts = proc_macro::TokenStream::from(quote!{
            impl ElementLife for #ident{
                fn construct(&mut self, cx: &mut Cx){
                    self.handle(cx, &Event::Construct);
                }

                fn destruct(&mut self, cx: &mut Cx){
                    self.handle(cx, &Event::Destruct);
                }

                fn update(&mut self, cx: &mut Cx){
                    self.handle(cx, &Event::Update);
                }
            }
        });
        return ts;
    };
    return proc_macro::TokenStream::new();
}