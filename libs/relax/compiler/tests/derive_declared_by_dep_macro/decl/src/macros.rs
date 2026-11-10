/// `decl_named!([Name, attributes(..)] => inner)`: the `#[proc_macro_derive]` fn `Name` of the
/// calling proc-macro crate, handing its input to `inner`.
#[macro_export]
macro_rules! decl_named {
    ([$derives:ident $($derive_t:tt)*] => $inner:path) => {
        #[proc_macro_derive($derives $($derive_t)*)]
        #[allow(non_snake_case)]
        pub fn $derives(i: proc_macro::TokenStream) -> proc_macro::TokenStream {
            $inner(i)
        }
    };
}
