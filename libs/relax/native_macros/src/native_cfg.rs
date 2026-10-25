//! The native expansions are certified by source identity (compiler native_macros.rs: the tree's
//! bytes equal the embedded ones). That certifies BEHAVIOUR only if every `cfg` in those sources
//! evaluates the same in relax's linked copy and in rustc's proc-macro build of the same package.
//! A `feature = ".."` predicate does not (the linked copy has the features of relax's own build:
//! `relax-tokens` on, while the graph's are only checked against an allowlist), nor does
//! `debug_assertions` (off in relax's release build, on in a dev-profile proc macro): the 10-25 incident was
//! `TokenParser::eat_type` reading path field types (`crate::a::B`) only under relax-tokens, so
//! `makepad check` accepted SandboxView's derive(Script) while rustc failed it ("Unexpected field
//! form" + 15 E0277). The build of relax-native-macros refuses such predicates, except the listed
//! token-model switch and runtime-only items that no expansion runs.
use relax_lex::tokens::{Delimiter, Span, TokenStream, TokenTree};

/// Cfg names whose value can differ between the linked copy and a rustc proc-macro build
/// (besides every `feature = ".."`).
const DIVERGENT_NAMES: &[&str] = &["debug_assertions"];

/// (package, file, feature): sites where a divergent predicate cannot change an expansion.
pub const ALLOWED: &[(&str, &str, &str)] = &[
    // the token-model switch itself: which `proc_macro` the parser and builder name
    ("makepad_micro_proc_macro", "src/lib.rs", "relax-tokens"),
    // the loader's runtime shim table (link sections); no macro expansion calls it
    ("makepad_relax_api", "src/lib.rs", "host-shims"),
];

/// The divergent `cfg` predicates of one Rust source: (line, the predicate's cfg name).
pub fn divergent_cfgs(src: &[u8]) -> Result<Vec<(usize, String)>, String> {
    let mut flat = Vec::new();
    relax_lex::lexer::lex(src, 2021, &mut flat).map_err(|_| "does not lex".to_string())?;
    let span_of = |lo: u32, hi: u32| Span { lo, hi, ..Span::DUMMY };
    let stream = relax_lex::tokens::from_toks(src, &flat, &span_of).map_err(|_| "unbalanced delimiters".to_string())?;
    let mut hits = Vec::new();
    walk(stream, &mut hits);
    Ok(hits.into_iter().map(|(lo, name)| (1 + src[..lo as usize].iter().filter(|b| **b == b'\n').count(), name)).collect())
}

fn walk(stream: TokenStream, hits: &mut Vec<(u32, String)>) {
    let trees: Vec<TokenTree> = stream.into_iter().collect();
    for (i, tree) in trees.iter().enumerate() {
        let TokenTree::Group(g) = tree else { continue };
        let prev = |k: usize| i.checked_sub(k).and_then(|j| trees.get(j));
        // `#[cfg(..)]` / `#[cfg_attr(..)]` / `#![..]`: the attribute's own stream
        let is_attr = g.delimiter() == Delimiter::Bracket && match (prev(1), prev(2)) {
            (Some(TokenTree::Punct(p)), _) if p.as_char() == '#' => true,
            (Some(TokenTree::Punct(b)), Some(TokenTree::Punct(h))) => b.as_char() == '!' && h.as_char() == '#',
            _ => false,
        };
        // `cfg!(..)`
        let is_cfg_macro = matches!((prev(1), prev(2)), (Some(TokenTree::Punct(b)), Some(TokenTree::Ident(n))) if b.as_char() == '!' && n.to_string() == "cfg");
        if is_attr {
            let inner: Vec<TokenTree> = g.stream().into_iter().collect();
            if let (Some(TokenTree::Ident(n)), Some(TokenTree::Group(args))) = (inner.first(), inner.get(1)) {
                if matches!(n.to_string().as_str(), "cfg" | "cfg_attr") { predicate(args.stream(), hits); }
            }
        } else if is_cfg_macro {
            predicate(g.stream(), hits);
        }
        walk(g.stream(), hits);
    }
}

fn predicate(stream: TokenStream, hits: &mut Vec<(u32, String)>) {
    let trees: Vec<TokenTree> = stream.into_iter().collect();
    for (i, tree) in trees.iter().enumerate() {
        match tree {
            TokenTree::Group(g) => predicate(g.stream(), hits),
            TokenTree::Ident(n) if DIVERGENT_NAMES.contains(&n.to_string().as_str()) => hits.push((n.span().lo, n.to_string())),
            TokenTree::Ident(n) if n.to_string() == "feature" => {
                if let (Some(TokenTree::Punct(eq)), Some(TokenTree::Literal(l))) = (trees.get(i + 1), trees.get(i + 2)) {
                    let value = l.to_string();
                    let value = value.trim_matches('"');
                    if eq.as_char() == '=' { hits.push((n.span().lo, value.to_string())); }
                }
            }
            _ => {}
        }
    }
}

/// Every divergent predicate of a package's files outside ALLOWED, as build errors.
pub fn check_package(package: &str, files: &[(String, Vec<u8>)]) -> Vec<String> {
    let mut errors = Vec::new();
    for (path, data) in files.iter().filter(|(p, _)| p.ends_with(".rs")) {
        match divergent_cfgs(data) {
            Err(e) => errors.push(format!("{package} {path}: {e}")),
            Ok(hits) => {
                for (line, name) in hits {
                    if !ALLOWED.iter().any(|(p, f, n)| *p == package && f == path && *n == name) {
                        errors.push(format!("{package} {path}:{line}: `cfg` on `{name}` differs between relax's linked copy and rustc's proc-macro build, so `makepad check` would expand this macro differently from cargo"));
                    }
                }
            }
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(path: &str, src: &str) -> Vec<(String, Vec<u8>)> { vec![(path.to_string(), src.as_bytes().to_vec())] }

    #[test]
    fn behaviour_gated_on_the_token_model_is_refused() {
        // the shape of work's eat_type before 10-25: a parse step only the linked copy had
        let src = "impl P { fn eat_type(&mut self) {\n #[cfg(feature = \"relax-tokens\")] while self.colons() {} } }";
        let errs = check_package("makepad_micro_proc_macro", &files("src/macro_lib.rs", src));
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert!(errs[0].contains("src/macro_lib.rs:2") && errs[0].contains("relax-tokens"), "{errs:?}");
        for src in ["#[cfg(all(test, not(feature = \"relax-tokens\")))] mod t {}", "fn f() -> bool { cfg!(debug_assertions) }",
                    "#![cfg_attr(debug_assertions, allow(dead_code))]", "#[cfg(any(feature = \"host-shims\", windows))] fn g() {}"] {
            assert_eq!(check_package("makepad_script_derive_impl", &files("src/lib.rs", src)).len(), 1, "{src}");
        }
    }

    #[test]
    fn the_token_model_switch_and_same_valued_cfgs_pass() {
        let switch = "#[cfg(not(feature = \"relax-tokens\"))] pub extern crate proc_macro;\n#[cfg(feature = \"relax-tokens\")] pub use relax_lex::tokens as proc_macro;";
        assert!(check_package("makepad_micro_proc_macro", &files("src/lib.rs", switch)).is_empty());
        // the same switch elsewhere is refused
        assert_eq!(check_package("makepad_micro_proc_macro", &files("src/other.rs", switch)).len(), 2);
        // cfgs with one value in both builds; strings that only mention a feature
        let same = "#[cfg(test)] mod t {} #[cfg(target_os = \"macos\")] fn m() {}\nconst C: &str = \"#[cfg(feature = \\\"host-shims\\\")]\";";
        assert!(check_package("makepad_relax_api_macro_impl", &files("src/lib.rs", same)).is_empty());
        // any feature: the graph's features are allowlisted, not compared with the linked copy's
        assert_eq!(check_package("makepad_derive_widget", &files("src/lib.rs", "#[cfg(feature = \"nightly\")] fn n() {}")).len(), 1);
    }
}
