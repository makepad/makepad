//! Source-matched native expansions. The compiler validates PACKAGES against the loaded
//! graph before using these functions; a different source version is a hard error.
use relax_base::pm::{Span, TokenStream};
mod sources;
#[cfg(test)]
mod native_cfg;
pub use sources::{PACKAGES, DEPENDENCIES};

#[derive(Clone, Copy)]
pub enum Function {
    Unary(fn(TokenStream) -> TokenStream),
    Attribute(fn(TokenStream, TokenStream) -> TokenStream),
}
pub struct Entry {
    pub krate: &'static str,
    pub fname: &'static str,
    pub function: Function,
    /// Opt-in contract supplied by the macro wrapper, never inferred by the host.
    pub declaration: Option<Function>,
}
impl Entry {
    pub fn call(&self, attr: Option<&TokenStream>, input: &TokenStream, span: Span) -> Result<TokenStream, String> {
        self.invoke(self.function, attr, input, span)
    }
    pub fn declarations(&self, attr: Option<&TokenStream>, input: &TokenStream, span: Span) -> Result<TokenStream, String> {
        let function = self.declaration.ok_or("macro has no declaration surface")?;
        self.invoke(function, attr, input, span)
    }
    fn invoke(&self, function: Function, attr: Option<&TokenStream>, input: &TokenStream, span: Span) -> Result<TokenStream, String> {
        struct Restore(Span);
        impl Drop for Restore {
            fn drop(&mut self) { Span::set_call_site(self.0); }
        }
        let _restore = Restore(Span::set_call_site(span));
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match (function, attr) {
            (Function::Unary(f), None) => Ok(f(input.clone())),
            (Function::Attribute(f), Some(attr)) => Ok(f(attr.clone(), input.clone())),
            _ => Err("native macro entry kind mismatch".to_string()),
        })).unwrap_or_else(|payload| Err(payload.downcast_ref::<String>().cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_else(|| "native macro panicked".to_string())))
    }
}
mod bindings;
pub use bindings::ENTRIES;



#[cfg(test)]
mod tests {
    use makepad_micro_proc_macro::{proc_macro::{Span, TokenStream}, TokenParser};
    use std::str::FromStr;

    /// The linked copy's token model takes the field types rustc's does (platform/script/tests/
    /// derive_path_types.rs is the rustc side): path types at a field's top level once failed
    /// derive(Script) with "Unexpected field form" in the rustc build only.
    #[test]
    fn struct_fields_take_path_types() {
        let src = "{ #[rust] a: crate::pad_routing::TickHold, #[rust] b: ::std::collections::BTreeMap<u32, self::x::Y>, \
                   #[live] c: Vec<crate::x::Y>, #[rust] d: Option<(String, String)>, #[rust] e: m::Wrap<T>::Inner }";
        // the host compiler installs the lexer behind `str::parse` (relax-front's builtins)
        fn lex(s: &str) -> Option<TokenStream> {
            let mut toks = Vec::new();
            relax_lex::lexer::lex(s.as_bytes(), 2021, &mut toks).ok()?;
            relax_lex::tokens::from_toks(s.as_bytes(), &toks, &|_, _| Span::call_site()).ok()
        }
        relax_lex::tokens::set_lexer(lex);
        let mut p = TokenParser::new(TokenStream::from_str(src).unwrap());
        let fields = p.eat_all_struct_fields().expect("fields parse");
        let got: Vec<(String, String)> = fields.iter().map(|f| (f.name.clone(), f.ty.to_string().replace(' ', ""))).collect();
        let want = [
            ("a", "crate::pad_routing::TickHold"),
            ("b", "::std::collections::BTreeMap<u32,self::x::Y>"),
            ("c", "Vec<crate::x::Y>"),
            ("d", "Option<(String,String)>"),
            ("e", "m::Wrap<T>::Inner"),
        ];
        assert_eq!(got, want.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect::<Vec<_>>());
    }
}

// This crate's source stamp (build.rs, libs/relax/self_stamp.rs): relax-compiler folds it into the codegen identity.
include!(concat!(env!("OUT_DIR"), "/src_stamp.rs"));
