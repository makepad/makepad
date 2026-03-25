use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{Attribute, FnArg, ItemFn, Pat, ReturnType};

#[proc_macro_attribute]
pub fn makepad_test(attr: TokenStream, item: TokenStream) -> TokenStream {
    match expand_makepad_test(attr.into(), item.into()) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.into_compile_error().into(),
    }
}

fn expand_makepad_test(
    attr: proc_macro2::TokenStream,
    item: proc_macro2::TokenStream,
) -> syn::Result<proc_macro2::TokenStream> {
    if !attr.is_empty() {
        return Err(syn::Error::new_spanned(
            attr,
            "#[makepad_test] does not accept arguments",
        ));
    }

    let mut function: ItemFn = syn::parse2(item)?;
    if function.sig.asyncness.is_some() {
        return Err(syn::Error::new_spanned(
            &function.sig.asyncness,
            "#[makepad_test] only supports synchronous tests",
        ));
    }
    if function.sig.constness.is_some() {
        return Err(syn::Error::new_spanned(
            &function.sig.constness,
            "#[makepad_test] does not support const functions",
        ));
    }
    if !function.sig.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &function.sig.generics,
            "#[makepad_test] does not support generic test functions",
        ));
    }
    if function.sig.inputs.len() != 1 {
        return Err(syn::Error::new_spanned(
            &function.sig.inputs,
            "#[makepad_test] expects exactly one TestApp argument",
        ));
    }

    let arg = function.sig.inputs.first().expect("checked arg length");
    let FnArg::Typed(arg) = arg else {
        return Err(syn::Error::new_spanned(
            arg,
            "#[makepad_test] does not support methods",
        ));
    };
    let Pat::Ident(_) = arg.pat.as_ref() else {
        return Err(syn::Error::new_spanned(
            &arg.pat,
            "#[makepad_test] requires an identifier pattern for the TestApp argument",
        ));
    };

    match &function.sig.output {
        ReturnType::Default => {}
        ReturnType::Type(_, _) => {}
    }

    let vis = function.vis.clone();
    let wrapper_attrs = function.attrs.clone();
    function
        .attrs
        .retain(|attr| !is_wrapper_only_test_attr(attr));
    let wrapper_name = function.sig.ident.clone();
    let inner_name = format_ident!("__makepad_test_inner_{}", wrapper_name);
    function.sig.ident = inner_name.clone();

    Ok(quote! {
        #function

        #(#wrapper_attrs)*
        #[test]
        #vis fn #wrapper_name() {
            ::makepad_test::__private::run_current_package_test(
                env!("CARGO_PKG_NAME"),
                env!("CARGO_MANIFEST_DIR"),
                module_path!(),
                stringify!(#wrapper_name),
                #inner_name,
            );
        }
    })
}

fn is_wrapper_only_test_attr(attr: &Attribute) -> bool {
    attr.path().is_ident("ignore") || attr.path().is_ident("should_panic")
}

#[cfg(test)]
mod tests {
    use super::expand_makepad_test;
    use quote::quote;
    use syn::{parse2, File, Item};

    #[test]
    fn expansion_wraps_test_body() {
        let output = expand_makepad_test(
            quote! {},
            quote! {
                fn return_submits(mut app: TestApp) {
                    app.press_return();
                }
            },
        )
        .unwrap()
        .to_string();

        assert!(output.contains("run_current_package_test"));
        assert!(output.contains("CARGO_PKG_NAME"));
        assert!(output.contains("CARGO_MANIFEST_DIR"));
        assert!(output.contains("module_path"));
        assert!(output.contains("__makepad_test_inner"));
    }

    #[test]
    fn expansion_rejects_arguments() {
        let err = expand_makepad_test(
            quote! {},
            quote! {
                fn bad(one: TestApp, two: TestApp) {}
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("exactly one TestApp argument"));
    }

    #[test]
    fn expansion_preserves_test_attrs_on_wrapper() {
        let output = expand_makepad_test(
            quote! {},
            quote! {
                #[cfg(target_os = "macos")]
                #[ignore]
                #[should_panic(expected = "boom")]
                fn smoke(app: TestApp) {}
            },
        )
        .unwrap();

        let file: File = parse2(output).expect("parse expansion");
        assert_eq!(file.items.len(), 2);

        let Item::Fn(inner) = &file.items[0] else {
            panic!("expected hidden inner function");
        };
        assert!(inner.attrs.iter().any(|attr| attr.path().is_ident("cfg")));
        assert!(!inner
            .attrs
            .iter()
            .any(|attr| attr.path().is_ident("ignore")));
        assert!(!inner
            .attrs
            .iter()
            .any(|attr| attr.path().is_ident("should_panic")));

        let Item::Fn(wrapper) = &file.items[1] else {
            panic!("expected wrapper test function");
        };
        assert!(wrapper.attrs.iter().any(|attr| attr.path().is_ident("cfg")));
        assert!(wrapper
            .attrs
            .iter()
            .any(|attr| attr.path().is_ident("ignore")));
        assert!(wrapper
            .attrs
            .iter()
            .any(|attr| attr.path().is_ident("should_panic")));
        assert!(wrapper
            .attrs
            .iter()
            .any(|attr| attr.path().is_ident("test")));
    }
}
