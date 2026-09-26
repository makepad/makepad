//! makepad-builder.exe carries the Builder icon, `resources/builder_1024.png`
//! (rendered from tools/app_icons/builder.svg), complete version information
//! and an asInvoker application manifest, as a well-formed Windows program
//! does. Other targets are unaffected.
use std::path::Path;

fn main() {
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let info = makepad_win_resource::VersionInfo {
        company_name: "Makepad".into(),
        file_description: "Makepad Builder".into(),
        product_name: "Makepad Builder".into(),
        internal_name: "makepad-builder".into(),
        original_filename: "makepad-builder.exe".into(),
        legal_copyright: "Copyright (c) Makepad".into(),
        version: makepad_win_resource::VersionInfo::parse_version(&version),
        version_text: version,
    };
    let icon = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/builder_1024.png");
    if let Err(error) = makepad_win_resource::link_app_resources_with(&icon, &info, Some(makepad_win_resource::AS_INVOKER_MANIFEST), Some("makepad-builder")) {
        panic!("Windows resources: {error}");
    }
}
