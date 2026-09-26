//! What one app's build says about that app: the package directory of a
//! packaged build (`MAKEPAD_PACKAGE_DIR`), its window icons
//! (`MAKEPAD_APP_ICON_*`) and, on macOS, its bundle name and identifier
//! (`MAKEPAD_BUNDLE_NAME` / `MAKEPAD_BUNDLE_IDENTIFIER`).
//!
//! These are read at compile time, but in the APP's crate, not in this one:
//! `app_main!` expands `option_env!` there, so rustc tracks the variables in
//! the app crate's dep-info. Building a bundle, or another app with other
//! icons, then recompiles that one crate. Read here (in platform's build
//! script or source), every change rebuilt platform and everything above it
//! (draw, widgets, every app library).

use std::sync::OnceLock;

include!(concat!(env!("OUT_DIR"), "/app_meta_gen.rs"));

/// The build inputs `app_main!` collects in the app crate.
#[doc(hidden)]
pub struct AppBuildMeta {
    /// `MAKEPAD_PACKAGE_DIR`: the build is packaged, its resources are here.
    pub package_dir: Option<&'static str>,
    /// `MAKEPAD_APP_ICON_{32,64,128,256,512,1024}`: window icon PNG files
    /// that replace the ones platform's build script found in the
    /// workspace's `resources/`. Read when the first window opens.
    pub icon_paths: [Option<&'static str>; 6],
}

static APP_BUILD_META: OnceLock<AppBuildMeta> = OnceLock::new();

/// Called by `app_main!` (through `_app_main_event_closure!`) before
/// `init_cx_os`. The first call wins.
#[doc(hidden)]
pub fn set_app_build_meta(meta: AppBuildMeta) {
    let _ = APP_BUILD_META.set(meta);
}

/// The package directory of a packaged build, `None` when the app runs
/// from its source checkout (or does not start through `app_main!`).
pub fn package_dir() -> Option<&'static str> {
    APP_BUILD_META.get().and_then(|meta| meta.package_dir)
}

#[allow(dead_code)]
pub(crate) fn icon_path(slot: usize) -> Option<&'static str> {
    APP_BUILD_META.get().and_then(|meta| meta.icon_paths.get(slot).copied().flatten())
}

/// The app crate's build inputs, as an [`AppBuildMeta`] expression.
#[doc(hidden)]
#[macro_export]
macro_rules! _app_build_meta {
    () => {
        $crate::app_meta::AppBuildMeta {
            package_dir: option_env!("MAKEPAD_PACKAGE_DIR"),
            icon_paths: [
                option_env!("MAKEPAD_APP_ICON_32"),
                option_env!("MAKEPAD_APP_ICON_64"),
                option_env!("MAKEPAD_APP_ICON_128"),
                option_env!("MAKEPAD_APP_ICON_256"),
                option_env!("MAKEPAD_APP_ICON_512"),
                option_env!("MAKEPAD_APP_ICON_1024"),
            ],
        }
    };
}

// The Info.plist `app_main!` embeds in a macOS executable's
// `__TEXT,__info_plist` section. CoreFoundation reads that section for an
// executable that is not inside a `.app` (a `cargo run` launch): macOS takes
// the application menu's title from its CFBundleName, whatever NSMenu title
// the app sets. The embedded plist wins over the `Info.plist` platform's
// build script writes beside the executables (the fallback for binaries that
// do not start through `app_main!`). In a `.app` the bundle's own
// Contents/Info.plist is used instead.

const INFO_PLIST_PARTS: [&str; 4] = [
    r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key>
    <string>"#,
    r#"</string>
    <key>CFBundleName</key>
    <string>"#,
    r#"</string>
    <key>CFBundleDisplayName</key>
    <string>"#,
    r#"</string>
    <key>GCSupportsControllerUserInteraction</key>
    <true/>
    <key>GCSupportedGameControllers</key>
    <array>
        <dict>
            <key>ProfileName</key>
            <string>ExtendedGamepad</string>
        </dict>
    </array>
    <key>NSLocationUsageDescription</key>
    <string>Used to show your position on the map.</string>
    <key>NSLocationWhenInUseUsageDescription</key>
    <string>Used to show your position on the map.</string>
</dict>
</plist>
"#,
];

const DEFAULT_IDENTIFIER_PREFIX: &str = "dev.makepad.";

const fn str_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// Writes `src` at `at` (when `out` is given) and returns the end offset.
/// `derive_id` lowercases ASCII and turns spaces into dashes, the default
/// identifier's spelling of a bundle name.
const fn put(out: &mut Option<&mut [u8]>, at: usize, src: &str, derive_id: bool) -> usize {
    let src = src.as_bytes();
    if let Some(out) = out {
        let mut i = 0;
        while i < src.len() {
            let mut b = src[i];
            if derive_id {
                if b == b' ' {
                    b = b'-';
                } else {
                    b = b.to_ascii_lowercase();
                }
            }
            out[at + i] = b;
            i += 1;
        }
    }
    at + src.len()
}

const fn write_info_plist(mut out: Option<&mut [u8]>, name: &str, identifier: Option<&str>) -> usize {
    let mut at = put(&mut out, 0, INFO_PLIST_PARTS[0], false);
    at = match identifier {
        Some(identifier) => put(&mut out, at, identifier, false),
        // The build script spelled the default name's identifier with
        // Unicode lowercasing; any other name gets the ASCII spelling.
        None if str_eq(name, DEFAULT_BUNDLE_NAME) => put(&mut out, at, DEFAULT_BUNDLE_IDENTIFIER, false),
        None => {
            let at = put(&mut out, at, DEFAULT_IDENTIFIER_PREFIX, false);
            put(&mut out, at, name, true)
        }
    };
    at = put(&mut out, at, INFO_PLIST_PARTS[1], false);
    at = put(&mut out, at, name, false);
    at = put(&mut out, at, INFO_PLIST_PARTS[2], false);
    at = put(&mut out, at, name, false);
    put(&mut out, at, INFO_PLIST_PARTS[3], false)
}

/// The byte length of [`info_plist`]`(name, identifier)`.
#[doc(hidden)]
pub const fn info_plist_len(name: &str, identifier: Option<&str>) -> usize {
    write_info_plist(None, name, identifier)
}

/// The Info.plist of an app named `name`; without an identifier it is
/// `dev.makepad.<name, lowercase, spaces as dashes>`.
#[doc(hidden)]
pub const fn info_plist<const N: usize>(name: &str, identifier: Option<&str>) -> [u8; N] {
    let mut out = [0u8; N];
    write_info_plist(Some(&mut out as &mut [u8]), name, identifier);
    out
}
