use std::env;
use std::fs::File;
use std::io::prelude::*;
use std::path::Path;

/// Best-effort default for the macOS bundle name when MAKEPAD_BUNDLE_NAME isn't
/// set. Cargo doesn't expose the consuming binary's package name to a
/// dependency's build script (`CARGO_PKG_NAME` here is "makepad-platform"), so
/// we walk up from `OUT_DIR` (which is always
/// `<root>/target/<profile>/build/<crate>-<hash>/out`) to the directory that
/// contains `target/` and use that directory's name. For a typical project
/// that's the package or workspace root, which is almost always a meaningful
/// label. Capitalize the first letter so the menu bar shows "Sample app" rather
/// than "sample app". Returns `None` if the path doesn't have the expected shape
/// or the directory name isn't valid UTF-8.
fn detect_app_name(out_dir: &Path) -> Option<String> {
    let workspace_root = out_dir.ancestors().nth(5)?;
    let dir_name = workspace_root.file_name()?.to_str()?;
    if dir_name.is_empty() {
        return None;
    }
    let mut chars = dir_name.chars();
    let first = chars.next()?;
    Some(first.to_uppercase().collect::<String>() + chars.as_str())
}

fn main() {
    println!("cargo:rustc-check-cfg=cfg(relax)");
    let out_dir = env::var("OUT_DIR").unwrap();
    let path = Path::new(&out_dir)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let cwd = std::env::current_dir().unwrap();
    let mut file = File::create(path.join("makepad-platform.path")).unwrap();
    file.write_all(format!("{}", cwd.display()).as_bytes())
        .unwrap();

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    let target = env::var("TARGET").unwrap();

    // The default macOS bundle name and identifier. An app overrides them
    // with MAKEPAD_BUNDLE_NAME / MAKEPAD_BUNDLE_IDENTIFIER (typically in its
    // `.cargo/config.toml` `[env]` section with `force = true`), which
    // `app_main!` reads in the APP crate and embeds in its executable (see
    // src/app_meta.rs): read here, a bundle build or another app's name
    // rebuilt this crate and everything above it. The default is the
    // workspace/package directory name (capitalized), which is almost always
    // more meaningful than a hardcoded placeholder; it depends only on where
    // the target directory is.
    let bundle_name = detect_app_name(Path::new(&out_dir)).unwrap_or_else(|| "Makepad App".to_string());
    let bundle_id = format!("dev.makepad.{}", bundle_name.to_lowercase().replace(' ', "-"));
    std::fs::write(
        Path::new(&out_dir).join("app_meta_gen.rs"),
        format!(
            "/// The bundle name when the app's build sets no MAKEPAD_BUNDLE_NAME.\n\
             pub const DEFAULT_BUNDLE_NAME: &str = {bundle_name:?};\n\
             /// The identifier that goes with [`DEFAULT_BUNDLE_NAME`].\n\
             pub const DEFAULT_BUNDLE_IDENTIFIER: &str = {bundle_id:?};\n"
        ),
    )
    .unwrap();

    if target_os == "macos" {
        // macOS uses CFBundleName from an Info.plist beside an unbundled
        // executable as the first menu bar item title for `cargo run`
        // launches, and it overrides whatever NSMenu title we pass to
        // setMainMenu:. `app_main!` embeds its own Info.plist in the
        // executable, which takes precedence; this one, with the defaults,
        // serves binaries that start without `app_main!`.
        let command_line_plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key>
    <string>{bundle_id}</string>
    <key>CFBundleName</key>
    <string>{bundle_name}</string>
    <key>CFBundleDisplayName</key>
    <string>{bundle_name}</string>
    <key>GCSupportsControllerUserInteraction</key>
    <true/>
    <key>GCSupportedGameControllers</key>
    <array>
        <dict>
            <key>ProfileName</key>
            <string>ExtendedGamepad</string>
        </dict>
    </array>
    <key>NSCameraUsageDescription</key>
    <string>Used to capture camera video when you request it.</string>
    <key>NSMicrophoneUsageDescription</key>
    <string>Used to capture microphone audio when you request it.</string>
    <key>NSLocationUsageDescription</key>
    <string>Used to show your position on the map.</string>
    <key>NSLocationWhenInUseUsageDescription</key>
    <string>Used to show your position on the map.</string>
</dict>
</plist>
"#
        );
        std::fs::write(path.join("Info.plist"), command_line_plist).unwrap();
    }

    // Window icons auto-discovered in `<workspace_root>/resources/`.
    // Workspace root is the dir containing `target/` (5 ancestors up from
    // OUT_DIR), same heuristic as `detect_app_name`. An app's own icons
    // (MAKEPAD_APP_ICON_*, which `cargo makepad desktop` sets per app) are
    // read by `app_main!` in the app crate instead (src/app_meta.rs), so
    // switching apps does not rebuild this crate.
    let icons: &[(&str, &str)] = &[
        ("icon_32.png", "CUSTOM_ICON_PNG_32"),
        ("icon_64.png", "CUSTOM_ICON_PNG_64"),
        ("icon_128.png", "CUSTOM_ICON_PNG_128"),
        ("icon_256.png", "CUSTOM_ICON_PNG_256"),
        ("icon_512.png", "CUSTOM_ICON_PNG_512"),
        ("icon_1024.png", "CUSTOM_ICON_PNG_1024"),
        ("icon.ico", "CUSTOM_ICON_ICO"),
    ];
    let resources_dir = Path::new(&out_dir).ancestors().nth(5).map(|r| r.join("resources"));
    let mut icon_gen = String::new();
    for &(filename, const_name) in icons {
        let path = resources_dir.as_ref().and_then(|dir| {
            let p = dir.join(filename);
            p.is_file().then(|| p.to_string_lossy().into_owned())
        });
        let value = match &path {
            Some(p) => {
                println!("cargo:rerun-if-changed={p}");
                format!("include_bytes!(r#\"{p}\"#)")
            }
            None => "&[]".to_string(),
        };
        icon_gen.push_str(&format!(
            "#[allow(dead_code)] pub static {const_name}: &'static [u8] = {value};\n"
        ));
    }
    // Watch the resources dir so new/removed icon files trigger a rebuild
    // (rerun-if-changed on a non-existent file is a no-op).
    if let Some(dir) = resources_dir.as_ref().filter(|d| d.is_dir()) {
        println!("cargo:rerun-if-changed={}", dir.display());
    }
    std::fs::write(Path::new(&out_dir).join("app_icon_gen.rs"), icon_gen).unwrap();

    println!("cargo:rustc-check-cfg=cfg(apple_bundle,apple_sim,lines,makepad_precompiled_shaders,makepad_shader_pack,makepad_no_studio)");
    // Every variable declared here rebuilds this crate and everything that
    // depends on it (draw, widgets, every app) when its value changes: keep
    // per-app inputs out (see src/app_meta.rs). MAKEPAD_BUILD selects build
    // options for bundle packaging and diagnostics; words that only concern a
    // crate further up belong in a variable of that crate's own. Never the
    // runtime MAKEPAD (vulkan, linux_direct, ...): a window manager, Code and a
    // terminal start the same `cargo run` with different values of it, and
    // each would rebuild every app.
    println!("cargo:rerun-if-env-changed=MAKEPAD_BUILD");
    println!("cargo:rerun-if-env-changed=IPHONEOS_DEPLOYMENT_TARGET");

    if let Ok(configs) = env::var("MAKEPAD_BUILD") {
        for config in configs.split(['+', ',']) {
            match config {
                "lines" => println!("cargo:rustc-cfg=lines"),
                "apple_bundle" => println!("cargo:rustc-cfg=apple_bundle"),
                _ => {}
            }
        }
    }

    match target_os.as_str() {
        // the frameworks are linked by the HAL's and the Apple bindings' own `#[link]`s
        "ios" => {
            if target == "aarch64-apple-ios-sim" {
                println!("cargo:rustc-cfg=apple_sim");
            }
        }
        "tvos" => {
            if target == "aarch64-apple-tvos-sim" {
                println!("cargo:rustc-cfg=apple_sim");
            }
        }
        _ => (),
    }
}
