//! makepad-builder.exe carries the Builder icon, `resources/builder_1024.png`
//! (rendered from tools/app_icons/builder.svg). Other targets are unaffected.
use std::path::Path;

fn main() {
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let info = makepad_win_resource::VersionInfo {
        file_description: "Makepad Builder".into(),
        product_name: "Makepad Builder".into(),
        internal_name: "makepad-builder".into(),
        original_filename: "makepad-builder.exe".into(),
        version: makepad_win_resource::VersionInfo::parse_version(&version),
        version_text: version,
        ..Default::default()
    };
    let icon = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/builder_1024.png");
    if let Err(error) = makepad_win_resource::link_app_resources(&icon, &info, Some("makepad-builder")) {
        panic!("Windows resources: {error}");
    }
}
