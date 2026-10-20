use std::env;
use std::fs::File;
use std::io::prelude::*;
use std::path::Path;

#[path = "build_gen.rs"]
mod build_gen;

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

    // The default macOS bundle name and identifier (build_gen.rs, shared with `makepad run`). An app
    // overrides them with MAKEPAD_BUNDLE_NAME / MAKEPAD_BUNDLE_IDENTIFIER (typically in its
    // `.cargo/config.toml` `[env]` section with `force = true`), which `app_main!` reads in the APP
    // crate and embeds in its executable (see src/app_meta.rs): read here, a bundle build or another
    // app's name rebuilt this crate and everything above it. The default depends only on where the
    // target directory is.
    let (bundle_name, bundle_id) = build_gen::bundle(Path::new(&out_dir));
    std::fs::write(Path::new(&out_dir).join("app_meta_gen.rs"), build_gen::app_meta_gen(Path::new(&out_dir))).unwrap();

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

    // Window icons auto-discovered in `<workspace_root>/resources/` (build_gen.rs).
    let icons = build_gen::app_icon_gen(Path::new(&out_dir));
    for p in &icons.files {
        println!("cargo:rerun-if-changed={}", p.display());
    }
    // Watch the resources dir so new/removed icon files trigger a rebuild
    // (rerun-if-changed on a non-existent file is a no-op).
    if let Some(dir) = &icons.dir {
        println!("cargo:rerun-if-changed={}", dir.display());
    }
    std::fs::write(Path::new(&out_dir).join("app_icon_gen.rs"), icons.source).unwrap();

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

    let makepad_build = env::var("MAKEPAD_BUILD").ok();
    for cfg in build_gen::cfgs(makepad_build.as_deref(), &target_os, &target) {
        println!("cargo:rustc-cfg={cfg}");
    }
}
