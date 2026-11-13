//! The generator of a native macro set (lib.rs `Set`), run by the build script of the crate that
//! provides the set: this crate's own (the base set) and a host's (a crate the host binary links and
//! registers through `relax_native_macros::register`). It includes this file with `#[path]` and
//! declares `sha256` and `self_stamp` at its root (self_stamp.rs), then calls [`generate`] with its
//! rows. The table is data; the code is this generator.
//!
//! Its outputs in OUT_DIR, which the providing crate `include!`s with `Package`, `Entry` and
//! `Function` in scope:
//! - `sources.rs`: `pub static PACKAGES: &[Package]`, each package's manifest, build script and
//!   source files as the build read them (the compiler checks the graph's against them, compiler
//!   native_macros.rs), its dependencies as its manifest declares them and the features a graph
//!   may ask of it;
//! - `bindings.rs`: the proc-macro wrappers of the packages that have a `module`, adapted from those
//!   same sources, and `pub static ENTRIES: &[Entry]`;
//! - `src_stamp.rs`: the providing crate's source stamp, over its own files, the embedded ones and
//!   this generator.
use relax_lex::tokens::{Delimiter, Span, TokenStream, TokenTree};
use std::path::PathBuf;

#[path = "../build_sources.rs"]
mod build_sources;
#[path = "src/native_cfg.rs"]
mod native_cfg;
#[path = "../base/src/toml/parser.rs"]
#[allow(dead_code)]
mod toml;

/// A package of a set.
pub struct Row {
    /// its crate name
    pub name: &'static str,
    /// its folder, relative to the providing crate's manifest folder
    pub dir: &'static str,
    /// a proc-macro crate: the module its wrappers become
    pub module: Option<&'static str>,
    /// the features a graph may build it with
    pub features: &'static [&'static str],
}

/// A package's dependencies as its manifest declares them (dev- and build-dependencies aside): each
/// crate name, and whether a graph may resolve without it (optional, or for some targets only).
fn manifest_deps(package: &str, manifest: &[u8]) -> Vec<(String, bool)> {
    let text = std::str::from_utf8(manifest).unwrap_or_else(|_| panic!("native macro package {package}: Cargo.toml is not UTF-8"));
    let doc = toml::parse_toml(text).unwrap_or_else(|e| panic!("native macro package {package}: Cargo.toml: {e}"));
    let mut deps: Vec<(String, bool)> = Vec::new();
    let mut add = |table: &toml::TomlTable, for_target: bool| {
        for (key, spec) in table {
            let optional = match spec {
                toml::Toml::Str(..) => false,
                toml::Toml::Table(spec) => {
                    // the compiler matches a dependency's alias with the crate it resolves to
                    assert!(!spec.contains_key("package"), "native macro package {package}: dependency {key} is renamed");
                    spec.get("optional").and_then(|o| o.as_bool()).unwrap_or(false)
                }
                _ => panic!("native macro package {package}: dependency {key} is neither a version nor a table"),
            };
            let name = key.replace('-', "_");
            let conditional = for_target || optional;
            match deps.iter_mut().find(|(n, _)| *n == name) {
                Some(dep) => dep.1 &= conditional,
                None => deps.push((name, conditional)),
            }
        }
    };
    if let Some(table) = doc.root.get("dependencies").and_then(|d| d.as_table()) {
        add(table, false);
    }
    for target in doc.root.get("target").and_then(|t| t.as_table()).into_iter().flat_map(|t| t.values()) {
        if let Some(table) = target.as_table().and_then(|t| t.get("dependencies")).and_then(|d| d.as_table()) {
            add(table, true);
        }
    }
    deps.sort();
    deps
}

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

/// Writes the set's `sources.rs`, `bindings.rs` and source stamp (module doc).
pub fn generate(rows: &[Row]) {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let mut packages = String::from("pub static PACKAGES: &[Package] = &[\n");
    let mut wrappers = String::from("use super::{Entry, Function};\n");
    let mut entries = String::new();
    let mut divergent = Vec::new();
    // the source stamp covers the proc-macro sources the crate embeds and this generator, all
    // outside the providing crate's own files
    let mut stamped: Vec<(String, Vec<u8>)> = [
        ("gen.rs", &include_bytes!("gen.rs")[..]),
        ("build_sources.rs", &include_bytes!("../build_sources.rs")[..]),
        ("native_cfg.rs", &include_bytes!("src/native_cfg.rs")[..]),
        ("toml_parser.rs", &include_bytes!("../base/src/toml/parser.rs")[..]),
    ].iter().map(|(name, bytes)| (format!("relax-native-macros/{name}"), bytes.to_vec())).collect();
    for row in rows {
        let root = manifest.join(row.dir).canonicalize().unwrap_or_else(|e| panic!("native macro package {} at {}: {e}", row.name, row.dir));
        let files = build_sources::package(&root);
        let cargo_toml = files.iter().find(|(path, _)| path == "Cargo.toml").unwrap_or_else(|| panic!("native macro package {} has no Cargo.toml", row.name));
        let deps = manifest_deps(row.name, &cargo_toml.1);
        packages.push_str(&format!("Package {{ name: {:?}, deps: &{:?}, features: &{:?}, files: &[\n", row.name, deps, row.features));
        divergent.extend(native_cfg::check_package(row.name, &files));
        for (path, data) in files {
            stamped.push((format!("{}/{path}", row.name), data.clone()));
            packages.push_str(&format!("({path:?}, include_bytes!({:?})),\n", root.join(&path)));
            if path == "src/lib.rs" {
                if let Some(module) = row.module {
                    wrappers.push_str(&bindings(&data, row.name, module, &mut entries));
                }
            }
        }
        packages.push_str("] },\n");
    }
    packages.push_str("];\n");
    // source identity certifies behaviour only when no cfg differs between this copy and rustc's
    assert!(divergent.is_empty(), "native proc-macro sources would expand differently under relax than under cargo:\n{}", divergent.join("\n"));
    wrappers.push_str(&format!("pub static ENTRIES: &[Entry] = &[\n{entries}];\n"));
    std::fs::write(out.join("sources.rs"), packages).expect("write native source identity");
    std::fs::write(out.join("bindings.rs"), wrappers).expect("write native wrappers");
    crate::self_stamp::write(&stamped);
}
