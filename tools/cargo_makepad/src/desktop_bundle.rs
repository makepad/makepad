//! `cargo makepad desktop bundle`: a self-contained, signed macOS `.app` for
//! a desktop app, optionally installed outside the target directory so a
//! Dock entry survives `cargo clean`.
//!
//! The app is built with `MAKEPAD_PACKAGE_DIR=resources` (in its own target
//! subdirectory, since that compile-time switch rebuilds the platform
//! crate), which makes it read its resources from inside the bundle rather
//! than from the source checkout:
//!
//! ```text
//! <Name>.app/Contents/
//!   Info.plist, PkgInfo
//!   MacOS/<bin>                              the executable
//!   MacOS/resources -> ../Resources          what MAKEPAD_PACKAGE_DIR names
//!   MacOS/<bin>.makepad-package-paths -> ../Resources/<bin>.makepad-package-paths
//!   Resources/AppIcon.icns
//!   Resources/<crate>/resources/...          every dependency's resources
//!   Resources/<crate>/fonts/...              only the fonts the binary uses
//!   Resources/<bin>.makepad-package-paths    crate name → resources/<crate>
//! ```
//!
//! The package map and its link are data, so they live in `Resources`: a
//! plain file in `MacOS` is taken for an unsigned nested code object.
//!
//! Name, identifier and icon come from `[package.metadata.makepad.desktop]`
//! in the app's `Cargo.toml`:
//!
//! ```toml
//! [package.metadata.makepad.desktop]
//! name = "Makepad Task Manager"
//! identifier = "nl.makepad.task"
//! icon = "resources/taskmanager-icon.png"   # square PNG, 1024 px is best
//!
//! # Privacy prompts, only for an app that asks for them:
//! [package.metadata.makepad.desktop.usage]
//! audio_capture = "Routes another app's sound through the equalizer."
//! ```
//!
//! `usage` takes `audio_capture`, `microphone` and `camera`, which become
//! the matching `NS...UsageDescription` keys; anything else is an error.
//!
//! Signing uses the one Apple Development identity in the keychain when
//! there is exactly one (or `--cert=`): macOS keys privacy grants such as
//! Full Disk Access on the signing identity, so they survive rebuilds. An
//! ad-hoc signature (`--adhoc`, or no identity found) works, but such grants
//! are tied to the exact build and are lost on the next one.

use crate::desktop::{codesign_path, resolve_codesign_identity};
use crate::font_assets::{is_font_path, FontAssetManifest, FontPackage};
use crate::makepad_shell::shell_env_cap;
use crate::utils::{get_build_crate_from_args, get_crate_dep_dirs, get_crate_dir, get_package_binary_name, get_profile_from_args, get_target_from_args};
use makepad_toml_parser::{parse_toml, Toml};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Where `--install` puts bundles when no directory is given.
const DEFAULT_INSTALL_DIR: &str = ".makepad/apps";

#[derive(Debug, Default)]
struct BundleOptions {
    /// `Some(dir)` installs the finished bundle into `dir`.
    install: Option<PathBuf>,
    cert: Option<String>,
    adhoc: bool,
}

/// What the app's manifest says about its bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BundleMeta {
    name: String,
    identifier: String,
    icon: Option<PathBuf>,
    /// Privacy prompts the app declares, as (Info.plist key, text), in
    /// [`USAGE_KEYS`] order.
    usage: Vec<(&'static str, String)>,
}

/// The privacy prompts an app may declare under
/// `[package.metadata.makepad.desktop.usage]`, by manifest name. Only an app
/// that names one gets its prompt: a key macOS finds in the plist is a
/// permission the app can ask for.
const USAGE_KEYS: [(&str, &str); 3] = [
    ("audio_capture", "NSAudioCaptureUsageDescription"),
    ("microphone", "NSMicrophoneUsageDescription"),
    ("camera", "NSCameraUsageDescription"),
];

pub fn handle_desktop_bundle(args: &[String]) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err("desktop bundle builds macOS .app bundles and runs on macOS only".to_string());
    }
    let (options, mut cargo_args) = parse_options(args)?;
    if get_target_from_args(&cargo_args).is_some() {
        return Err("desktop bundle builds for this Mac; drop --target".to_string());
    }
    if !cargo_args.iter().any(|arg| arg == "--release" || arg.starts_with("--profile")) {
        cargo_args.push("--release".to_string());
    }
    let build_crate = get_build_crate_from_args(&cargo_args)?.to_string();
    let binary_name = get_package_binary_name(&build_crate).unwrap_or_else(|| build_crate.clone());
    if !cargo_args.iter().any(|arg| arg == "--bin" || arg.starts_with("--bin=")) {
        cargo_args.push("--bin".to_string());
        cargo_args.push(binary_name.clone());
    }
    let crate_dir = get_crate_dir(&build_crate)?;
    let meta = read_bundle_meta(&crate_dir, &build_crate, &binary_name)?;
    let profile = get_profile_from_args(&cargo_args);
    let target_dir = bundle_target_dir();

    println!("[cargo-makepad] building {build_crate} for {} ({})", meta.name, meta.identifier);
    let status = Command::new("cargo")
        .arg("build")
        .args(&cargo_args)
        .env("CARGO_TARGET_DIR", &target_dir)
        .env("MAKEPAD_PACKAGE_DIR", "resources")
        .env("MAKEPAD_BUNDLE_NAME", &meta.name)
        .env("MAKEPAD_BUNDLE_IDENTIFIER", &meta.identifier)
        .env_remove("MAKEPAD")
        .env_remove("CARGO_BUILD_TARGET")
        .status()
        .map_err(|e| format!("failed to run cargo build: {e}"))?;
    if !status.success() {
        return Err(format!("cargo build failed with status {status}"));
    }
    let built = target_dir.join(&profile).join(&binary_name);
    if !built.is_file() {
        return Err(format!("built binary not found at {}", built.display()));
    }

    let out_dir = target_dir.join("bundles").join(&profile);
    let app_dir = out_dir.join(format!("{}.app", meta.name));
    let stage = out_dir.join(format!(".{}.app.staging", meta.name));
    remove_dir_if_present(&stage)?;
    assemble(&stage, &built, &binary_name, &build_crate, &crate_dir, &target_dir, &meta)?;

    let identity = if options.adhoc {
        "-".to_string()
    } else {
        match resolve_codesign_identity(options.cert.as_deref()) {
            Ok(identity) => identity,
            Err(reason) if options.cert.is_none() => {
                eprintln!("warning: {reason}; signing ad hoc, so privacy grants (Full Disk Access, camera, ...) will not survive a rebuild");
                "-".to_string()
            }
            Err(reason) => return Err(reason),
        }
    };
    codesign_path(&stage.join("Contents/MacOS").join(&binary_name), &identity, None, false)?;
    codesign_path(&stage, &identity, None, false)?;
    verify_signature(&stage)?;
    replace_bundle(&stage, &app_dir, &meta.identifier)?;
    println!("[cargo-makepad] macOS app bundle: {}", app_dir.display());
    println!("[cargo-makepad] signed with: {}", if identity == "-" { "ad hoc" } else { &identity });

    if let Some(dir) = options.install {
        let installed = install(&app_dir, &dir, &meta)?;
        println!("[cargo-makepad] installed: {}", installed.display());
    }
    Ok(())
}

fn parse_options(args: &[String]) -> Result<(BundleOptions, Vec<String>), String> {
    let mut options = BundleOptions::default();
    let mut cargo_args = Vec::new();
    for arg in args {
        if arg == "--install" {
            options.install = Some(default_install_dir()?);
        } else if let Some(dir) = arg.strip_prefix("--install=") {
            if dir.is_empty() {
                return Err("--install= needs a directory".to_string());
            }
            options.install = Some(expand_home(dir)?);
        } else if let Some(cert) = arg.strip_prefix("--cert=") {
            options.cert = Some(cert.to_string());
        } else if arg == "--adhoc" {
            options.adhoc = true;
        } else {
            cargo_args.push(arg.clone());
        }
    }
    if options.adhoc && options.cert.is_some() {
        return Err("use either --adhoc or --cert=<identity>, not both".to_string());
    }
    Ok((options, cargo_args))
}

fn home_dir() -> Result<PathBuf, String> {
    std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| "HOME is not set".to_string())
}

fn default_install_dir() -> Result<PathBuf, String> {
    Ok(home_dir()?.join(DEFAULT_INSTALL_DIR))
}

fn expand_home(dir: &str) -> Result<PathBuf, String> {
    match dir.strip_prefix("~/") {
        Some(rest) => Ok(home_dir()?.join(rest)),
        None if dir == "~" => home_dir(),
        None => Ok(PathBuf::from(dir)),
    }
}

/// The bundle build's own target directory: `MAKEPAD_PACKAGE_DIR` is read at
/// compile time, so sharing the normal one would rebuild the platform crate
/// every time a plain build and a bundle build alternate.
fn bundle_target_dir() -> PathBuf {
    let base = match std::env::var_os("CARGO_TARGET_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from("target"),
    };
    let base = if base.is_absolute() { base } else { std::env::current_dir().unwrap_or_default().join(base) };
    base.join("makepad-bundle")
}

fn read_bundle_meta(crate_dir: &Path, build_crate: &str, binary_name: &str) -> Result<BundleMeta, String> {
    let manifest = crate_dir.join("Cargo.toml");
    let text = fs::read_to_string(&manifest).map_err(|e| format!("cannot read {}: {e}", manifest.display()))?;
    let toml = parse_toml(&text).map_err(|e| format!("cannot parse {}: {}", manifest.display(), e.msg))?;
    bundle_meta_from(&toml, crate_dir, build_crate, binary_name)
}

/// `[package.metadata.makepad.desktop]`, then the `packager` names other
/// cargo-makepad targets read, then names made from the crate.
fn bundle_meta_from(toml: &makepad_toml_parser::TomlDocument, crate_dir: &Path, build_crate: &str, binary_name: &str) -> Result<BundleMeta, String> {
    let text = |path: &[&str]| match toml.get_path(path) {
        Some(Toml::Str(value, _)) if !value.trim().is_empty() => Some(value.trim().to_string()),
        _ => None,
    };
    let name = text(&["package", "metadata", "makepad", "desktop", "name"])
        .or_else(|| text(&["package", "metadata", "packager", "product_name"]))
        .unwrap_or_else(|| binary_name.to_string());
    let identifier = text(&["package", "metadata", "makepad", "desktop", "identifier"])
        .or_else(|| text(&["package", "metadata", "packager", "identifier"]))
        .unwrap_or_else(|| format!("dev.makepad.{build_crate}"));
    let icon = text(&["package", "metadata", "makepad", "desktop", "icon"]).map(|icon| crate_dir.join(icon));
    let mut usage = Vec::new();
    match toml.get_path(&["package", "metadata", "makepad", "desktop", "usage"]) {
        None => {}
        Some(Toml::Table(table)) => {
            for name in table.keys() {
                if !USAGE_KEYS.iter().any(|(known, _)| known == name) {
                    let known: Vec<&str> = USAGE_KEYS.iter().map(|(known, _)| *known).collect();
                    return Err(format!("unknown usage key {name:?} in [package.metadata.makepad.desktop.usage]; known: {}", known.join(", ")));
                }
            }
            for (name, key) in USAGE_KEYS {
                match table.get(name) {
                    None => {}
                    Some(Toml::Str(text, _)) if !text.trim().is_empty() => usage.push((key, text.trim().to_string())),
                    Some(_) => return Err(format!("usage.{name} must be a non-empty string")),
                }
            }
        }
        Some(_) => return Err("[package.metadata.makepad.desktop.usage] must be a table".to_string()),
    }
    Ok(BundleMeta { name, identifier, icon, usage })
}

/// Build the whole bundle at `stage`.
fn assemble(stage: &Path, built: &Path, binary_name: &str, build_crate: &str, crate_dir: &Path, target_dir: &Path, meta: &BundleMeta) -> Result<(), String> {
    if meta.name.contains('/') || meta.name.starts_with('.') {
        return Err(format!("bundle name {:?} cannot be a file name", meta.name));
    }
    let contents = stage.join("Contents");
    let macos = contents.join("MacOS");
    let resources = contents.join("Resources");
    fs::create_dir_all(&macos).map_err(|e| format!("cannot create {}: {e}", macos.display()))?;
    fs::create_dir_all(&resources).map_err(|e| format!("cannot create {}: {e}", resources.display()))?;

    let exe = macos.join(binary_name);
    fs::copy(built, &exe).map_err(|e| format!("cannot copy {} into the bundle: {e}", built.display()))?;

    // Every crate's resources, and only the fonts the binary declares.
    let manifest = FontAssetManifest::from_native_file(built)?;
    let mut fonts = FontPackage::new(&manifest);
    let host = host_triple()?;
    let mut crates: Vec<(String, PathBuf)> = vec![(build_crate.to_string(), crate_dir.to_path_buf())];
    let mut deps: Vec<(String, PathBuf)> = get_crate_dep_dirs(build_crate, target_dir, &host).into_iter().collect();
    deps.sort();
    crates.extend(deps);
    let mut mapped: Vec<String> = Vec::new();
    for (name, dir) in &crates {
        let name = name.replace('-', "_");
        if mapped.contains(&name) {
            continue;
        }
        let dest = resources.join(&name);
        let resource_dir = dir.join("resources");
        fonts.copy_tree_filtered(&resource_dir, &dest.join("resources"), &format!("{name}/resources"), |relative| relative.starts_with("android"), |_| Ok(()))?;
        // A font kept under fonts/ that also sits in resources/ is already there.
        fonts.copy_tree_filtered(&dir.join("fonts"), &dest.join("fonts"), &format!("{name}/fonts"), |relative| !is_font_path(relative) || resource_dir.join(relative).is_file(), |_| Ok(()))?;
        if dest.is_dir() {
            mapped.push(name);
        }
    }
    fonts.finish()?.print();

    // A bin target's own crate name is the binary's, which is what its
    // `crate://self` paths name; it maps to the package's resources.
    let own = build_crate.replace('-', "_");
    let alias = binary_name.replace('-', "_");
    let mut map = String::new();
    for name in &mapped {
        map.push_str(&format!("{name}\tresources/{name}\n"));
    }
    if alias != own && mapped.contains(&own) && !mapped.contains(&alias) {
        map.push_str(&format!("{alias}\tresources/{own}\n"));
    }
    let map_name = format!("{binary_name}.makepad-package-paths");
    fs::write(resources.join(&map_name), map).map_err(|e| format!("cannot write the package map: {e}"))?;
    symlink(&format!("../Resources/{map_name}"), &macos.join(&map_name))?;
    symlink("../Resources", &macos.join("resources"))?;

    write_icon(&resources.join("AppIcon.icns"), crate_dir, meta, stage)?;
    let plist = contents.join("Info.plist");
    fs::write(&plist, info_plist(binary_name, meta)).map_err(|e| format!("cannot write Info.plist: {e}"))?;
    shell_env_cap(&[], stage, "plutil", &["-lint", &plist.to_string_lossy()])?;
    fs::write(contents.join("PkgInfo"), "APPL????").map_err(|e| format!("cannot write PkgInfo: {e}"))?;
    Ok(())
}

fn host_triple() -> Result<String, String> {
    let cwd = std::env::current_dir().unwrap_or_default();
    let info = shell_env_cap(&[], &cwd, "rustc", &["-vV"])?;
    info.lines()
        .find_map(|line| line.strip_prefix("host: "))
        .map(|host| host.trim().to_string())
        .ok_or_else(|| "rustc -vV names no host".to_string())
}

fn symlink(target: &str, link: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).map_err(|e| format!("cannot link {} -> {target}: {e}", link.display()))
    }
    #[cfg(not(unix))]
    {
        let _ = (target, link);
        Err("bundles need symbolic links".to_string())
    }
}

/// The icon: the manifest's PNG resampled to an `.icns`, else a ready
/// `resources/icon.icns`, else cargo-makepad's own icon files.
fn write_icon(dest: &Path, crate_dir: &Path, meta: &BundleMeta, stage: &Path) -> Result<(), String> {
    if let Some(source) = &meta.icon {
        if !source.is_file() {
            return Err(format!("icon {} (from [package.metadata.makepad.desktop]) does not exist", source.display()));
        }
        return icns_from_png(source, dest, stage);
    }
    let ready = crate_dir.join("resources/icon.icns");
    if ready.is_file() {
        return fs::copy(&ready, dest).map(|_| ()).map_err(|e| format!("cannot copy {}: {e}", ready.display()));
    }
    for name in ["icon_1024.png", "icon_512.png", "icon_256.png"] {
        let png = crate_dir.join("resources").join(name);
        if png.is_file() {
            return icns_from_png(&png, dest, stage);
        }
    }
    eprintln!("warning: no icon: set icon in [package.metadata.makepad.desktop] or add resources/icon.icns");
    Ok(())
}

/// Every size an `.icns` carries, resampled from one PNG with `sips`, then
/// packed with `iconutil`. The source file is only read.
fn icns_from_png(source: &Path, dest: &Path, stage: &Path) -> Result<(), String> {
    let set = stage.join("AppIcon.iconset");
    fs::create_dir_all(&set).map_err(|e| format!("cannot create {}: {e}", set.display()))?;
    let source = source.to_string_lossy().to_string();
    for (name, size) in [
        ("icon_16x16.png", 16),
        ("icon_16x16@2x.png", 32),
        ("icon_32x32.png", 32),
        ("icon_32x32@2x.png", 64),
        ("icon_128x128.png", 128),
        ("icon_128x128@2x.png", 256),
        ("icon_256x256.png", 256),
        ("icon_256x256@2x.png", 512),
        ("icon_512x512.png", 512),
        ("icon_512x512@2x.png", 1024),
    ] {
        let size = size.to_string();
        let out = set.join(name).to_string_lossy().to_string();
        shell_env_cap(&[], stage, "sips", &["-s", "format", "png", "-z", &size, &size, &source, "--out", &out])?;
    }
    shell_env_cap(&[], stage, "iconutil", &["-c", "icns", &set.to_string_lossy(), "-o", &dest.to_string_lossy()])?;
    fs::remove_dir_all(&set).map_err(|e| format!("cannot remove {}: {e}", set.display()))
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn info_plist(binary_name: &str, meta: &BundleMeta) -> String {
    let exe = xml_escape(binary_name);
    let id = xml_escape(&meta.identifier);
    let name = xml_escape(&meta.name);
    let usage: String = meta.usage.iter().map(|(key, text)| format!("    <key>{key}</key>\n    <string>{}</string>\n", xml_escape(text))).collect();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">
<plist version=\"1.0\">
<dict>
    <key>CFBundleExecutable</key>
    <string>{exe}</string>
    <key>CFBundleIdentifier</key>
    <string>{id}</string>
    <key>CFBundleName</key>
    <string>{name}</string>
    <key>CFBundleDisplayName</key>
    <string>{name}</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon.icns</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleShortVersionString</key>
    <string>1.0</string>
    <key>CFBundleVersion</key>
    <string>1.0</string>
    <key>LSUIElement</key>
    <false/>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSPrincipalClass</key>
    <string>NSApplication</string>
{usage}</dict>
</plist>
"
    )
}

fn verify_signature(bundle: &Path) -> Result<(), String> {
    let cwd = std::env::current_dir().unwrap_or_default();
    shell_env_cap(&[], &cwd, "codesign", &["--verify", "--deep", "--strict", &bundle.to_string_lossy()]).map(|_| ())
}

fn bundle_identifier(bundle: &Path) -> Option<String> {
    let cwd = std::env::current_dir().unwrap_or_default();
    let plist = bundle.join("Contents/Info.plist");
    shell_env_cap(&[], &cwd, "plutil", &["-extract", "CFBundleIdentifier", "raw", "-o", "-", &plist.to_string_lossy()])
        .ok()
        .map(|id| id.trim().to_string())
}

/// Put `stage` at `dest`, replacing a previous bundle of the same app and
/// refusing to replace anything else. The old bundle is only removed once
/// the new one is in place, and is put back if the swap fails.
fn replace_bundle(stage: &Path, dest: &Path, identifier: &str) -> Result<(), String> {
    if dest.exists() {
        let existing = bundle_identifier(dest);
        if existing.as_deref() != Some(identifier) {
            return Err(format!(
                "refusing to replace {} (identifier {}, expected {identifier})",
                dest.display(),
                existing.unwrap_or_else(|| "unreadable".to_string())
            ));
        }
    }
    let Some(parent) = dest.parent() else { return Err(format!("{} has no parent directory", dest.display())) };
    let Some(file_name) = dest.file_name() else { return Err(format!("{} has no file name", dest.display())) };
    let previous = parent.join(format!(".{}.previous", file_name.to_string_lossy()));
    remove_dir_if_present(&previous)?;
    if dest.exists() {
        fs::rename(dest, &previous).map_err(|e| format!("cannot move {} aside: {e}", dest.display()))?;
        if let Err(error) = fs::rename(stage, dest) {
            let _ = fs::rename(&previous, dest);
            return Err(format!("cannot put the new bundle at {}: {error}; the previous one is back", dest.display()));
        }
        remove_dir_if_present(&previous)?;
    } else {
        fs::rename(stage, dest).map_err(|e| format!("cannot put the new bundle at {}: {e}", dest.display()))?;
    }
    Ok(())
}

/// Copy the signed bundle into `dir` with `ditto`, which keeps the links,
/// modes and signature, then swap it in like a fresh build.
fn install(app_dir: &Path, dir: &Path, meta: &BundleMeta) -> Result<PathBuf, String> {
    fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let dest = dir.join(format!("{}.app", meta.name));
    let stage = dir.join(format!(".{}.app.staging", meta.name));
    remove_dir_if_present(&stage)?;
    let cwd = std::env::current_dir().unwrap_or_default();
    shell_env_cap(&[], &cwd, "ditto", &[&app_dir.to_string_lossy(), &stage.to_string_lossy()])?;
    verify_signature(&stage)?;
    replace_bundle(&stage, &dest, &meta.identifier)?;
    Ok(dest)
}

/// Remove one of our own staging or previous bundles: only a path this
/// module named, ending in `.staging` or `.previous`.
fn remove_dir_if_present(path: &Path) -> Result<(), String> {
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    if !(name.ends_with(".staging") || name.ends_with(".previous")) {
        return Err(format!("refusing to remove {}", path.display()));
    }
    match fs::symlink_metadata(path) {
        Ok(_) => fs::remove_dir_all(path).map_err(|e| format!("cannot remove {}: {e}", path.display())),
        Err(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta_of(manifest: &str) -> BundleMeta {
        let toml = parse_toml(manifest).unwrap();
        bundle_meta_from(&toml, Path::new("/src/app"), "makepad-demo", "demo").unwrap()
    }

    #[test]
    fn the_desktop_table_names_the_bundle() {
        let meta = meta_of(
            "[package]\nname = \"makepad-demo\"\n\n[package.metadata.makepad.desktop]\nname = \"Makepad Demo\"\nidentifier = \"nl.makepad.demo\"\nicon = \"resources/demo.png\"\n",
        );
        assert_eq!(meta, BundleMeta { name: "Makepad Demo".into(), identifier: "nl.makepad.demo".into(), icon: Some(PathBuf::from("/src/app/resources/demo.png")), usage: Vec::new() });
    }

    #[test]
    fn without_a_desktop_table_the_packager_names_or_the_crate_are_used() {
        let meta = meta_of("[package]\nname = \"makepad-demo\"\n\n[package.metadata.packager]\nproduct_name = \"Demo\"\nidentifier = \"rs.demo\"\n");
        assert_eq!(meta, BundleMeta { name: "Demo".into(), identifier: "rs.demo".into(), icon: None, usage: Vec::new() });
        let meta = meta_of("[package]\nname = \"makepad-demo\"\n");
        assert_eq!(meta, BundleMeta { name: "demo".into(), identifier: "dev.makepad.makepad-demo".into(), icon: None, usage: Vec::new() });
    }

    #[test]
    fn options_split_from_cargo_args() {
        let args: Vec<String> = ["-p", "makepad-task", "--install=/tmp/apps", "--adhoc"].iter().map(|s| s.to_string()).collect();
        let (options, cargo) = parse_options(&args).unwrap();
        assert_eq!(options.install, Some(PathBuf::from("/tmp/apps")));
        assert!(options.adhoc);
        assert_eq!(cargo, vec!["-p".to_string(), "makepad-task".to_string()]);
        let args: Vec<String> = ["--adhoc", "--cert=X"].iter().map(|s| s.to_string()).collect();
        assert!(parse_options(&args).is_err());
    }

    #[test]
    fn usage_prompts_are_opt_in_and_allowlisted() {
        let meta = meta_of("[package]\nname = \"makepad-demo\"\n\n[package.metadata.makepad.desktop.usage]\naudio_capture = \"Routes <sound> & more\"\n");
        assert_eq!(meta.usage, vec![("NSAudioCaptureUsageDescription", "Routes <sound> & more".to_string())]);
        let plist = info_plist("demo", &meta);
        assert!(plist.contains("<key>NSAudioCaptureUsageDescription</key>\n    <string>Routes &lt;sound&gt; &amp; more</string>"));
        // No table, no prompt.
        let plain = meta_of("[package]\nname = \"makepad-demo\"\n");
        assert!(!info_plist("demo", &plain).contains("UsageDescription"));
        // A key outside the allowlist, or one that is not text, is refused.
        let bad = parse_toml("[package.metadata.makepad.desktop.usage]\nlocation = \"x\"\n").unwrap();
        assert!(bundle_meta_from(&bad, Path::new("/src"), "d", "d").is_err());
        let bad = parse_toml("[package.metadata.makepad.desktop.usage]\ncamera = 1\n").unwrap();
        assert!(bundle_meta_from(&bad, Path::new("/src"), "d", "d").is_err());
    }

    #[test]
    fn plist_text_is_escaped() {
        let meta = BundleMeta { name: "A & B <C>".into(), identifier: "x.y".into(), icon: None, usage: Vec::new() };
        let plist = info_plist("demo", &meta);
        assert!(plist.contains("<string>A &amp; B &lt;C&gt;</string>"));
    }

    #[test]
    fn only_our_own_staging_directories_are_removed() {
        assert!(remove_dir_if_present(Path::new("/Applications")).is_err());
        assert!(remove_dir_if_present(Path::new("/nonexistent/.X.app.staging")).is_ok());
    }
}
