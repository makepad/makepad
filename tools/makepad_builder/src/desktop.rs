//! Portable macOS application bundles for locally compiled apps.
use crate::catalog::Release;
use std::{fs, os::unix::fs::symlink, path::{Path, PathBuf}, time::UNIX_EPOCH};

fn xml(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
        .replace('"', "&quot;").replace('\'', "&apos;")
}

/// Return the bundle's launch wrapper. The original root binary and command
/// remain available; resources stay in the downloaded source repositories.
pub fn prepare(root: &Path, release: &Release, project: &Path) -> Result<PathBuf, String> {
    let root = crate::validate_install_root(root)?;
    prepare_inner(&root, release, project).map_err(|e| format!("Prepare macOS application: {e}"))
}

fn prepare_inner(root: &Path, release: &Release, project: &Path) -> std::io::Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let root = root.canonicalize()?;
    let project = project.canonicalize()?;
    let binary = &release.binary;
    let mut chars = binary.chars();
    let name = chars.next().unwrap().to_uppercase().collect::<String>() + chars.as_str();
    let bundle = root.join(format!("{name}.app"));
    let executable = bundle.join("Contents/MacOS").join(binary);
    fs::create_dir_all(root.join("installed"))?;
    // User state lives outside the bundle. It is relative for projects inside
    // the portable installation, and never interpolated into shell code.
    fs::write(root.join("installed").join(format!("{binary}.project")),
        project.strip_prefix(&root).unwrap_or(&project).to_string_lossy().as_bytes())?;

    let source = root.join(format!("{binary}.bin"));
    let metadata = fs::metadata(&source)?;
    let map = fs::read_to_string(root.join(format!("{binary}.bin.makepad-package-paths")))?;
    let launcher = include_str!("../macos-launcher.sh").replace("@APP_BINARY@", binary);
    let stamp = format!("bundle-v1\n{}\n{:?}\n{}\n{launcher}\n{map}", metadata.len(),
        metadata.modified()?.duration_since(UNIX_EPOCH).unwrap_or_default(), release.title);
    if fs::read_to_string(bundle.join("Contents/Resources/build-receipt")).ok().as_deref() == Some(&stamp)
        && executable.is_file()
    {
        return Ok(executable);
    }

    let stage = root.join(format!(".{name}.app.next"));
    if stage.exists() { fs::remove_dir_all(&stage)?; }
    struct Stage(PathBuf);
    impl Drop for Stage { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }
    let _stage = Stage(stage.clone());
    let macos = stage.join("Contents/MacOS");
    let resources = stage.join("Contents/Resources");
    fs::create_dir_all(&macos)?;
    fs::create_dir_all(&resources)?;
    fs::copy(&source, macos.join(format!("{binary}-bin")))?;
    fs::write(macos.join(binary), launcher)?;
    fs::set_permissions(macos.join(binary), fs::Permissions::from_mode(0o755))?;
    // The resource loader intentionally accepts only normal path components.
    // A relative installation link preserves that contract without hardcoding
    // the original folder or copying all fonts/icons into the application.
    symlink("../../..", macos.join("installation"))?;
    let paths: String = map.lines().filter_map(|line| line.split_once('\t'))
        .map(|(name, path)| format!("{name}\tinstallation/{path}\n")).collect();
    fs::write(macos.join(format!("{binary}-bin.makepad-package-paths")), paths)?;

    let app_resources = release.source(&root).join("resources");
    let icon = if release.id == "scope" {
        fs::write(resources.join("AppIcon.icns"), include_bytes!("../resources/scope.icns"))?;
        "AppIcon.icns"
    } else if app_resources.join("icon.icns").is_file() {
        fs::copy(app_resources.join("icon.icns"), resources.join("AppIcon.icns"))?;
        "AppIcon.icns"
    } else if app_resources.join("icon_1024.png").is_file() {
        fs::copy(app_resources.join("icon_1024.png"), resources.join("AppIcon.png"))?;
        "AppIcon.png"
    } else { "" };
    let icon = if icon.is_empty() { String::new() } else {
        format!("<key>CFBundleIconFile</key><string>{icon}</string>")
    };
    let plist = format!(r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>{binary}</string>
<key>CFBundleIdentifier</key><string>nl.makepad.{id}</string>
<key>CFBundleName</key><string>{title}</string>
<key>CFBundleDisplayName</key><string>{title}</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleVersion</key><string>1.0</string>
<key>CFBundleShortVersionString</key><string>1.0</string>
<key>LSUIElement</key><false/>
<key>NSHighResolutionCapable</key><true/>
<key>NSLocationUsageDescription</key><string>Used to show your position on the map.</string>
<key>NSMicrophoneUsageDescription</key><string>Used for audio input in Makepad apps.</string>
<key>NSCameraUsageDescription</key><string>Used for camera input in Makepad apps.</string>
{icon}
</dict></plist>
"#, id=release.id.replace('_', "-"), title=xml(&release.title));
    fs::write(stage.join("Contents/Info.plist"), plist)?;
    fs::write(stage.join("Contents/PkgInfo"), b"APPL????")?;
    fs::write(resources.join("build-receipt"), stamp)?;
    let previous = root.join(format!(".{name}.app.previous"));
    if previous.exists() { fs::remove_dir_all(&previous)?; }
    let existed = bundle.exists();
    if existed { fs::rename(&bundle, &previous)?; }
    if let Err(error) = fs::rename(&stage, &bundle) {
        if existed { let _ = fs::rename(&previous, &bundle); }
        return Err(error);
    }
    if existed { fs::remove_dir_all(previous)?; }
    Ok(executable)
}
