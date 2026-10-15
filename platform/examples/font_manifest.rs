//! Prints the font asset manifest `app_main!` links for a font set (`Latin`, the default, or
//! `International`): `cargo makepad android run-host` packages the fonts of an app it does not
//! build with rustc (the app is compiled on the phone), so it asks the platform for the set.

use makepad_platform::{INTERNATIONAL_FONT_ASSET_MANIFEST, LATIN_FONT_ASSET_PACKAGE_MANIFEST};

fn main() {
    let set = std::env::args().nth(1).unwrap_or_else(|| "Latin".to_string());
    let manifest: &[u8] = match set.as_str() {
        "Latin" => LATIN_FONT_ASSET_PACKAGE_MANIFEST,
        "International" => INTERNATIONAL_FONT_ASSET_MANIFEST,
        other => {
            eprintln!("font_manifest: unknown font set {other:?} (Latin or International)");
            std::process::exit(2);
        }
    };
    print!("{}", String::from_utf8_lossy(manifest));
}
