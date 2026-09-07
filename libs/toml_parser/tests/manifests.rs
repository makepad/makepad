//! Real manifests from this repository, parsed and spot-checked.
use makepad_toml_parser::{parse_toml, Toml, TomlDocument};

fn parse(text: &str) -> TomlDocument {
    parse_toml(text).unwrap_or_else(|e| panic!("manifest should parse: {e:?}"))
}

fn get_str<'a>(doc: &'a TomlDocument, path: &[&str]) -> Option<&'a str> {
    doc.get_path(path).and_then(Toml::as_str)
}

#[test]
fn root_workspace_manifest() {
    let doc = parse(include_str!("../../../Cargo.toml"));
    let members = doc
        .get_path(&["workspace", "members"])
        .and_then(Toml::as_array)
        .expect("workspace.members");
    assert!(members.iter().any(|m| m.as_str() == Some("apps/studio")));
    assert!(members.iter().any(|m| m.as_str() == Some("apps/terminal")));
    assert!(members.len() > 50);
}

#[test]
fn studio_manifest_has_four_bins_and_a_lib() {
    let doc = parse(include_str!("../../../apps/studio/Cargo.toml"));
    assert_eq!(get_str(&doc, &["package", "name"]), Some("makepad-studio"));
    assert_eq!(get_str(&doc, &["lib", "name"]), Some("makepad_studio"));
    let bins = doc
        .get_path(&["bin"])
        .and_then(Toml::as_array_of_tables)
        .expect("[[bin]] entries");
    let names: Vec<&str> = bins.iter().map(|b| b["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        ["studio", "studio-git-guard", "studio-rustc-guard", "studio-flow"]
    );
    assert_eq!(bins[1]["test"].as_bool(), Some(false));
    assert_eq!(bins[0]["path"].as_str(), Some("src/main.rs"));
    assert_eq!(
        get_str(&doc, &["dependencies", "makepad-widgets", "path"]),
        Some("../../widgets")
    );
}

#[test]
fn terminal_manifest() {
    let doc = parse(include_str!("../../../apps/terminal/Cargo.toml"));
    assert_eq!(get_str(&doc, &["package", "name"]), Some("makepad-terminal"));
    assert!(doc.get_path(&["dependencies"]).and_then(Toml::as_table).is_some());
}

#[test]
fn git_manifest() {
    let doc = parse(include_str!("../../../libs/git/Cargo.toml"));
    assert_eq!(get_str(&doc, &["package", "name"]), Some("makepad-git"));
    assert_eq!(
        get_str(&doc, &["dependencies", "makepad-fast-inflate", "path"]),
        Some("../fast_inflate")
    );
}

#[test]
fn package_metadata_through_dotted_keys() {
    let doc = parse(include_str!("../../../tools/cargo_makepad/Cargo.toml"));
    assert_eq!(get_str(&doc, &["package", "name"]), Some("cargo-makepad"));
    assert_eq!(
        get_str(&doc, &["package", "metadata", "makepad-check-platform"]),
        Some("desktop")
    );
    assert!(get_str(&doc, &["package", "metadata", "makepad-auto-version"]).is_some());

    let doc = parse(include_str!("../../../libs/rust_tokenizer/Cargo.toml"));
    assert_eq!(
        get_str(&doc, &["package", "metadata", "makepad-auto-version"]),
        Some("oD0L5c3G2VuWsqkFkpgKSat_PMo=")
    );
    assert_eq!(
        get_str(&doc, &["dependencies", "makepad-live-id", "version"]),
        Some("1.0.0")
    );
}
