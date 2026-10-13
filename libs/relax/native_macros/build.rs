use relax_lex::tokens::{Delimiter, Span, TokenStream, TokenTree};
use std::path::PathBuf;

#[path = "../build_sources.rs"]
mod build_sources;

// Package, Makepad path, and (for proc-macro wrappers) the generated module.
const PACKAGES: &[(&str, &str, Option<&str>)] = &[
    ("makepad_micro_proc_macro", "libs/micro_proc_macro", None),
    ("makepad_live_id", "libs/live_id", None),
    ("makepad_live_id_macros", "libs/live_id/id_macros", Some("id")),
    ("makepad_live_id_macros_impl", "libs/live_id/id_macros_impl", None),
    ("makepad_micro_serde_derive", "libs/micro_serde/derive", Some("serde")),
    ("makepad_micro_serde_derive_impl", "libs/micro_serde/derive_impl", None),
    ("makepad_script_derive", "platform/script/derive", Some("script")),
    ("makepad_script_derive_impl", "platform/script/derive_impl", None),
    ("makepad_derive_widget", "widgets/derive_widget", Some("widget")),
    ("makepad_derive_widget_impl", "widgets/derive_widget_impl", None),
    ("makepad_relax_api", "libs/relax_api", None),
    ("makepad_relax_api_macro", "libs/relax_api/macro", Some("relax_api")),
    ("makepad_relax_api_macro_impl", "libs/relax_api/macro_impl", None),
];

fn ident(token: Option<&TokenTree>, name: &str) -> bool {
    matches!(token, Some(TokenTree::Ident(i)) if i.to_string() == name)
}

/// Adapt the actual wrapper source at every build. Only the proc-macro annotations and
/// token import change; wrapper bodies cannot drift from the embedded source identity.
fn bindings(source: &[u8], krate: &str, module: &str, entries: &mut String) -> String {
    let mut flat = Vec::new();
    relax_lex::lexer::lex(source, 2021, &mut flat).unwrap_or_else(|_| panic!("cannot lex proc-macro wrapper in {krate}"));
    let stream = relax_lex::tokens::from_toks(source, &flat, &|_, _| Span::DUMMY).expect("parse proc-macro wrapper");
    let tokens: Vec<_> = stream.into_iter().collect();
    let mut adapted = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        if matches!(&tokens[i], TokenTree::Punct(p) if p.as_char() == '#') {
            if let Some(TokenTree::Group(attr)) = tokens.get(i + 1) {
                if attr.delimiter() == Delimiter::Bracket {
                    let first = attr.stream().into_iter().next();
                    if ["proc_macro", "proc_macro_derive", "proc_macro_attribute"].iter().any(|name| ident(first.as_ref(), name)) {
                        assert!(ident(tokens.get(i + 2), "pub") && ident(tokens.get(i + 3), "fn"), "unsupported proc-macro wrapper in {krate}");
                        let Some(TokenTree::Ident(fname)) = tokens.get(i + 4) else { panic!("missing proc-macro name in {krate}") };
                        let kind = if ident(first.as_ref(), "proc_macro_attribute") { "Attribute" } else { "Unary" };
                        let declaration_name = format!("declaration_{fname}");
                        let declaration = if tokens.windows(2).any(|pair| ident(pair.first(), "fn") && ident(pair.get(1), &declaration_name)) {
                            format!("Some(Function::{kind}({module}::{declaration_name}))")
                        } else { "None".into() };
                        entries.push_str(&format!("Entry {{ krate: {krate:?}, fname: {:?}, function: Function::{kind}({module}::{fname}), declaration: {declaration} }},\n", fname.to_string()));
                        i += 2;
                        continue;
                    }
                }
            }
        }
        if ident(tokens.get(i), "extern") && ident(tokens.get(i + 1), "crate") && ident(tokens.get(i + 2), "proc_macro") {
            assert!(matches!(tokens.get(i + 3), Some(TokenTree::Punct(p)) if p.as_char() == ';'), "unsupported proc_macro import in {krate}");
            i += 4;
            continue;
        }
        if ident(tokens.get(i), "fn") && matches!(tokens.get(i + 1), Some(TokenTree::Ident(name)) if name.to_string().starts_with("declaration_")) {
            adapted.push(TokenTree::Ident(relax_lex::tokens::Ident::new("pub", Span::DUMMY)));
        }
        adapted.push(tokens[i].clone());
        i += 1;
    }
    let adapted: TokenStream = adapted.into_iter().collect();
    format!("mod {module} {{ use makepad_micro_proc_macro::proc_macro; {adapted} }}\n")
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../build_sources.rs");
    let makepad = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../../..").canonicalize().expect("Makepad checkout");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let mut packages = String::from("pub static PACKAGES: &[(&str, &[(&str, &[u8])])] = &[\n");
    let mut wrappers = String::from("use super::{Entry, Function};\n");
    let mut entries = String::new();
    for (name, relative, module) in PACKAGES {
        let root = makepad.join(relative);
        packages.push_str(&format!("({name:?}, &[\n"));
        for (path, data) in build_sources::package(&root) {
            packages.push_str(&format!("({path:?}, include_bytes!({:?})),\n", root.join(&path)));
            if path == "src/lib.rs" {
                if let Some(module) = module {
                    wrappers.push_str(&bindings(&data, name, module, &mut entries));
                }
            }
        }
        packages.push_str("]),\n");
    }
    packages.push_str("];\n");
    wrappers.push_str(&format!("pub static ENTRIES: &[Entry] = &[\n{entries}];\n"));
    std::fs::write(out.join("sources.rs"), packages).expect("write native source identity");
    std::fs::write(out.join("bindings.rs"), wrappers).expect("write native wrappers");
}
