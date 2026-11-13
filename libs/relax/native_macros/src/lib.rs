//! Source-matched native expansions, in sets: this crate's (`BASE`, Relax's own macros and the
//! carried libraries they use) and those a host binary links and registers (`register`; a set's
//! crate generates it with gen.rs). The compiler validates a set's packages against the loaded
//! graph before using its functions; a different source version is a hard error.
use relax_base::pm::{Span, TokenStream};
#[cfg(test)]
mod native_cfg;

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

/// A package a set runs natively (its proc macros, or a library of theirs): its crate name, its
/// dependencies as its manifest declares them (crate name, and whether a graph may resolve without
/// it: optional, or for some targets only), the features a graph may build it with, and its files
/// as the set's build read them (path in the package, bytes).
pub struct Package {
    pub name: &'static str,
    pub deps: &'static [(&'static str, bool)],
    pub features: &'static [&'static str],
    pub files: &'static [(&'static str, &'static [u8])],
}

/// A set of native proc macros: its packages, their entries, and the source stamp of the crate that
/// provides it (the codegen identity holds it, compiler codegen_id.rs).
pub struct Set {
    pub name: &'static str,
    pub packages: &'static [Package],
    pub entries: &'static [Entry],
    pub stamp: u128,
}

mod bindings;
// This crate's source stamp (gen.rs, libs/relax/self_stamp.rs): relax-compiler folds it into the codegen identity.
include!(concat!(env!("OUT_DIR"), "/src_stamp.rs"));
// The base set's packages, as the build read them (gen.rs).
include!(concat!(env!("OUT_DIR"), "/sources.rs"));

/// The base set.
pub static BASE: Set = Set { name: "relax", packages: PACKAGES, entries: bindings::ENTRIES, stamp: SRC_STAMP };

/// The sets a host registered, and whether the sets were read (then none is added any more).
static REGISTERED: std::sync::Mutex<(bool, Vec<&'static Set>)> = std::sync::Mutex::new((false, Vec::new()));
static SETS: std::sync::OnceLock<Vec<&'static Set>> = std::sync::OnceLock::new();

/// Adds a host's set (relax_runner's `Host::native_sets`). Before anything reads the sets: the
/// compiler's validation and the codegen identity would not see a later one, so it panics.
pub fn register(set: &'static Set) {
    let mut registered = REGISTERED.lock().unwrap_or_else(|e| e.into_inner());
    assert!(!registered.0, "native macro set `{}` registered after the sets were read", set.name);
    registered.1.push(set);
}

/// The base set and every registered one, by name. The first read fixes them for the process.
pub fn sets() -> &'static [&'static Set] {
    SETS.get_or_init(|| {
        let mut registered = REGISTERED.lock().unwrap_or_else(|e| e.into_inner());
        registered.0 = true;
        let mut all: Vec<&'static Set> = std::iter::once(&BASE).chain(registered.1.iter().copied()).collect();
        all.sort_by_key(|s| s.name);
        let mut names = std::collections::HashSet::new();
        for s in &all {
            assert!(names.insert(s.name), "two native macro sets are named `{}`", s.name);
        }
        let mut packages = std::collections::HashSet::new();
        for p in all.iter().flat_map(|s| s.packages) {
            assert!(packages.insert(p.name), "native macro package `{}` is in two sets", p.name);
        }
        all
    })
}



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
