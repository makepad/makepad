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

    if target_os == "macos" {
        // The downstream app can override the bundle name shown in the macOS
        // application menu by setting MAKEPAD_BUNDLE_NAME — typically via its
        // `.cargo/config.toml` `[env]` section with `force = true`. macOS uses
        // CFBundleName from this Info.plist as the first menu bar item title
        // for unbundled `cargo run` launches, and it overrides whatever NSMenu
        // title we pass to setMainMenu:. When the env var isn't set, we fall
        // back to the workspace/package directory name (capitalized), which
        // is almost always more meaningful than a hardcoded placeholder.
        let bundle_name = env::var("MAKEPAD_BUNDLE_NAME")
            .ok()
            .or_else(|| detect_app_name(Path::new(&out_dir)))
            .unwrap_or_else(|| "Makepad App".to_string());
        let bundle_id = env::var("MAKEPAD_BUNDLE_IDENTIFIER")
            .unwrap_or_else(|_| format!("dev.makepad.{}", bundle_name.to_lowercase().replace(' ', "-")));
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
</dict>
</plist>
"#
        );
        std::fs::write(path.join("Info.plist"), command_line_plist).unwrap();
    }

    // Per slot: env-var override → auto-discovery in `<workspace_root>/resources/`.
    // Workspace root is the dir containing `target/` (5 ancestors up from
    // OUT_DIR), same heuristic as `detect_app_name`.
    let icons: &[(&str, &str, &str)] = &[
        ("MAKEPAD_APP_ICON_32",   "icon_32.png",   "CUSTOM_ICON_PNG_32"),
        ("MAKEPAD_APP_ICON_64",   "icon_64.png",   "CUSTOM_ICON_PNG_64"),
        ("MAKEPAD_APP_ICON_128",  "icon_128.png",  "CUSTOM_ICON_PNG_128"),
        ("MAKEPAD_APP_ICON_256",  "icon_256.png",  "CUSTOM_ICON_PNG_256"),
        ("MAKEPAD_APP_ICON_512",  "icon_512.png",  "CUSTOM_ICON_PNG_512"),
        ("MAKEPAD_APP_ICON_1024", "icon_1024.png", "CUSTOM_ICON_PNG_1024"),
        ("MAKEPAD_APP_ICON_ICO",  "icon.ico",      "CUSTOM_ICON_ICO"),
    ];
    let resources_dir = Path::new(&out_dir).ancestors().nth(5).map(|r| r.join("resources"));
    let mut icon_gen = String::new();
    for &(var, filename, const_name) in icons {
        println!("cargo:rerun-if-env-changed={var}");
        let path = env::var(var).ok().or_else(|| {
            let p = resources_dir.as_ref()?.join(filename);
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

    println!("cargo:rustc-check-cfg=cfg(apple_bundle,apple_sim,lines,use_gles_3,use_vulkan,linux_direct,quest,no_android_choreographer,ohos_sim,headless,use_unstable_unix_socket_ancillary_data_2021)");
    println!("cargo:rerun-if-env-changed=MAKEPAD");
    println!("cargo:rerun-if-env-changed=MAKEPAD_PACKAGE_DIR");
    println!("cargo:rerun-if-env-changed=MAKEPAD_BUNDLE_NAME");
    println!("cargo:rerun-if-env-changed=MAKEPAD_BUNDLE_IDENTIFIER");
    println!("cargo:rerun-if-env-changed=IPHONEOS_DEPLOYMENT_TARGET");

    if let Ok(configs) = env::var("MAKEPAD") {
        for config in configs.split(['+', ',']) {
            match config {
                "lines" => println!("cargo:rustc-cfg=lines"),
                "linux_direct" => println!("cargo:rustc-cfg=linux_direct"),
                "no_android_choreographer" => println!("cargo:rustc-cfg=no_android_choreographer"),
                "quest" => {
                    println!("cargo:rustc-cfg=quest");
                    println!("cargo:rustc-cfg=use_gles_3");
                    println!("cargo:rustc-cfg=use_vulkan");
                }
                "apple_bundle" => println!("cargo:rustc-cfg=apple_bundle"),
                "ohos_sim" => println!("cargo:rustc-cfg=ohos_sim"),
                "headless" => println!("cargo:rustc-cfg=headless"),
                "use_gles_3" => println!("cargo:rustc-cfg=use_gles_3"),
                "vulkan" | "use_vulkan" => println!("cargo:rustc-cfg=use_vulkan"),
                _ => {}
            }
        }
    }

    match target_os.as_str() {
        "macos" => {
            println!("cargo:rustc-link-lib=framework=GameController");
        }
        "ios" => {
            if target == "aarch64-apple-ios-sim" {
                println!("cargo:rustc-cfg=apple_sim");
            }
            println!("cargo:rustc-link-lib=framework=MetalKit");
            println!("cargo:rustc-link-lib=framework=GameController");
        }
        "tvos" => {
            if target == "aarch64-apple-tvos-sim" {
                println!("cargo:rustc-cfg=apple_sim");
            }
            println!("cargo:rustc-link-lib=framework=MetalKit");
            println!("cargo:rustc-link-lib=framework=GameController");
        }
        "linux" => {
            println!("cargo:rustc-cfg=use_gles_3");
            println!("cargo:rustc-link-lib=xkbcommon");
        }
        "android" => {
            println!("cargo:rustc-cfg=use_gles_3");
        }
        _ => (),
    }
}
