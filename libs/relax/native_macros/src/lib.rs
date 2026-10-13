//! Source-matched native expansions. The compiler validates PACKAGES against the loaded
//! graph before using these functions; a different source version is a hard error.
use relax_base::pm::{Span, TokenStream};
mod sources;
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


