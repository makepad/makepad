//! What makepad-platform's build script generates, in one place: `platform/build.rs` (cargo) includes
//! this file with `#[path]`, and `makepad run` (libs/relax/front/src/unitgraph.rs) includes the same file
//! and answers the script natively, without booting a second Relax process for it. Plain std code, no
//! cargo calls: the two callers cannot drift (a test in relax-front runs the real build.rs against it).
#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// Default bundle name when MAKEPAD_BUNDLE_NAME isn't set. Cargo doesn't expose the consuming
/// binary's package name to a dependency's build script (`CARGO_PKG_NAME` here is
/// "makepad-platform"), so walk up from `OUT_DIR` (always
/// `<root>/target/<profile>/build/<crate>-<hash>/out`) to the directory that contains `target/`
/// and use that directory's name, first letter capitalised. None if the path doesn't have the
/// expected shape or the directory name isn't valid UTF-8.
pub fn detect_app_name(out_dir: &Path) -> Option<String> {
    let workspace_root = out_dir.ancestors().nth(5)?;
    let dir_name = workspace_root.file_name()?.to_str()?;
    if dir_name.is_empty() {
        return None;
    }
    let mut chars = dir_name.chars();
    let first = chars.next()?;
    Some(first.to_uppercase().collect::<String>() + chars.as_str())
}

/// (bundle name, bundle identifier) of the build.
pub fn bundle(out_dir: &Path) -> (String, String) {
    let name = detect_app_name(out_dir).unwrap_or_else(|| "Makepad App".to_string());
    let id = format!("dev.makepad.{}", name.to_lowercase().replace(' ', "-"));
    (name, id)
}

/// The text of OUT_DIR/app_meta_gen.rs.
pub fn app_meta_gen(out_dir: &Path) -> String {
    let (bundle_name, bundle_id) = bundle(out_dir);
    format!(
        "/// The bundle name when the app's build sets no MAKEPAD_BUNDLE_NAME.\n\
         pub const DEFAULT_BUNDLE_NAME: &str = {bundle_name:?};\n\
         /// The identifier that goes with [`DEFAULT_BUNDLE_NAME`].\n\
         pub const DEFAULT_BUNDLE_IDENTIFIER: &str = {bundle_id:?};\n"
    )
}

/// Window icons auto-discovered in `<workspace_root>/resources/` (the dir containing `target/`,
/// 5 ancestors up from OUT_DIR, as in `detect_app_name`). An app's own icons (MAKEPAD_APP_ICON_*)
/// are read by `app_main!` in the app crate instead (src/app_meta.rs).
pub struct Icons {
    /// the text of OUT_DIR/app_icon_gen.rs
    pub source: String,
    /// the icon files it includes
    pub files: Vec<PathBuf>,
    /// `<root>/resources` when it is a directory (cargo watches it)
    pub dir: Option<PathBuf>,
}

pub fn app_icon_gen(out_dir: &Path) -> Icons {
    let icons: &[(&str, &str)] = &[
        ("icon_32.png", "CUSTOM_ICON_PNG_32"),
        ("icon_64.png", "CUSTOM_ICON_PNG_64"),
        ("icon_128.png", "CUSTOM_ICON_PNG_128"),
        ("icon_256.png", "CUSTOM_ICON_PNG_256"),
        ("icon_512.png", "CUSTOM_ICON_PNG_512"),
        ("icon_1024.png", "CUSTOM_ICON_PNG_1024"),
        ("icon.ico", "CUSTOM_ICON_ICO"),
    ];
    let resources_dir = out_dir.ancestors().nth(5).map(|r| r.join("resources"));
    let mut source = String::new();
    let mut files = Vec::new();
    for &(filename, const_name) in icons {
        let path = resources_dir.as_ref().and_then(|dir| {
            let p = dir.join(filename);
            p.is_file().then_some(p)
        });
        let value = match &path {
            Some(p) => {
                files.push(p.clone());
                format!("include_bytes!(r#\"{}\"#)", p.to_string_lossy())
            }
            None => "&[]".to_string(),
        };
        source.push_str(&format!("#[allow(dead_code)] pub static {const_name}: &'static [u8] = {value};\n"));
    }
    Icons { source, files, dir: resources_dir.filter(|d| d.is_dir()) }
}

/// The `cargo:rustc-cfg=` words: the `lines` / `apple_bundle` words of MAKEPAD_BUILD (split on
/// `+` and `,`) and `apple_sim` for the iOS / tvOS simulator targets.
pub fn cfgs(makepad_build: Option<&str>, target_os: &str, target: &str) -> Vec<&'static str> {
    let mut out = Vec::new();
    if let Some(configs) = makepad_build {
        for config in configs.split(['+', ',']) {
            match config {
                "lines" => out.push("lines"),
                "apple_bundle" => out.push("apple_bundle"),
                _ => {}
            }
        }
    }
    match target_os {
        "ios" if target == "aarch64-apple-ios-sim" => out.push("apple_sim"),
        "tvos" if target == "aarch64-apple-tvos-sim" => out.push("apple_sim"),
        _ => {}
    }
    out
}
