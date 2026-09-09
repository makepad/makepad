use super::AppleOs;
use makepad_micro_serde::{DeJson, DeJsonErr, DeJsonState, JsonValue};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(test)]
mod tests;

pub(super) struct InfoPlist {
    path: PathBuf,
    xml: String,
}

#[derive(DeJson)]
struct CargoMetadata {
    packages: Vec<CargoPackage>,
}

#[derive(DeJson)]
struct CargoPackage {
    name: String,
    version: String,
    id: String,
    manifest_path: String,
    metadata: JsonValue,
}

/// Load an optional app-owned plist relative to the selected Cargo package,
/// never the invoking workspace directory. iOS and tvOS configure it separately.
pub(super) fn load(cwd: &Path, build_crate: &str, os: AppleOs) -> Result<Option<InfoPlist>, String> {
    // Let Cargo parse the manifest and resolve the selected workspace member.
    // A file URL from `cargo pkgid` is not a filesystem path (e.g. `%20`).
    let output = Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("Cannot read Cargo metadata for {}: {e}", cwd.join("Cargo.toml").display()))?;
    if !output.status.success() {
        return Err(format!("Cannot read Cargo metadata for {}: {}",
            cwd.join("Cargo.toml").display(), String::from_utf8_lossy(&output.stderr)));
    }
    let json = std::str::from_utf8(&output.stdout)
        .map_err(|e| format!("Cargo metadata is not UTF-8: {e}"))?;
    let metadata = CargoMetadata::deserialize_json_lenient(json)
        .map_err(|e| format!("Cannot parse Cargo metadata: {e:?}"))?;
    let package = metadata.packages.iter().find(|package| {
        package.name == build_crate || package.id == build_crate
            || format!("{}@{}", package.name, package.version) == build_crate
    }).ok_or_else(|| format!("Cargo metadata does not contain package {build_crate}"))?;
    let manifest = Path::new(&package.manifest_path);
    let crate_dir = manifest.parent()
        .ok_or_else(|| format!("Cargo returned an invalid manifest path: {}", manifest.display()))?;
    let platform = match os {
        AppleOs::Ios => "ios",
        AppleOs::Tvos => "tvos",
    };
    let key = format!("package.metadata.makepad.{platform}.info_plist");
    let configured = package.metadata.key("makepad")
        .and_then(|value| value.key(platform))
        .and_then(|value| value.key("info_plist"));
    let path = match configured {
        None => return Ok(None),
        Some(JsonValue::String(path)) if !path.is_empty() => crate_dir.join(path),
        _ => return Err(format!("{key} in {} must be a nonempty path string", manifest.display())),
    };
    let xml = read_dictionary(&path)
        .map_err(|e| format!("Cannot read custom Info.plist {}: {e}", path.display()))?;
    Ok(Some(InfoPlist { path, xml }))
}

impl InfoPlist {
    /// Replace whole top-level values, preserving types and unspecified defaults.
    /// Bundle identity and executable stay controlled by the build's CLI inputs:
    /// changing them here would disagree with signing and simulator launch paths.
    pub(super) fn merge(&self, generated: &str) -> Result<String, String> {
        self.merge_inner(generated)
            .map_err(|e| format!("Cannot merge custom Info.plist {}: {e}", self.path.display()))
    }

    fn merge_inner(&self, generated: &str) -> Result<String, String> {
        let temporary = TemporaryDirectory::new()?;
        let defaults = temporary.0.join("defaults.plist");
        let merged = temporary.0.join("merged.plist");
        fs::write(&defaults, generated.trim_start())
            .map_err(|e| format!("Cannot write generated Info.plist: {e}"))?;
        read_dictionary(&defaults)?;
        fs::write(&merged, &self.xml)
            .map_err(|e| format!("Cannot write temporary Info.plist: {e}"))?;

        // PlistBuddy's Merge adds only missing top-level keys. Starting with the
        // app's dictionary therefore preserves whole custom values, including
        // nested dictionaries and arrays. Capture its duplicate-key notices.
        // Fixed filenames in a private directory avoid interpolating app paths
        // into PlistBuddy's command language; the source file is never modified.
        run(Command::new("/usr/libexec/PlistBuddy")
            .current_dir(&temporary.0)
            .args(["-c", "Merge defaults.plist", "merged.plist"]))?;

        for key in ["CFBundleIdentifier", "CFBundleExecutable"] {
            // Compare typed, canonical XML so e.g. a boolean cannot pass as a
            // string. Check after merging so absent custom keys need no special
            // handling, and all command failures remain errors.
            let extract = |path: &Path| run(Command::new("/usr/bin/plutil")
                .args(["-extract", key, "xml1", "-o", "-", "--"])
                .arg(path));
            if extract(&merged)? != extract(&defaults)? {
                return Err(format!(
                    "cannot change {key}; use the Apple build options and Cargo binary target instead",
                ));
            }
        }
        read_dictionary(&merged)
    }
}

/// Apple builds already require macOS and its plist utilities. Let plutil parse
/// both XML and binary input; only inspect its normalized XML to check the root
/// type, without implementing a general XML or binary plist parser.
fn read_dictionary(path: &Path) -> Result<String, String> {
    let xml = run(Command::new("/usr/bin/plutil")
        .args(["-convert", "xml1", "-o", "-", "--"])
        .arg(path))?;
    let root = xml.split_once("<plist")
        .and_then(|(_, rest)| rest.split_once('>'))
        .map(|(_, rest)| rest.trim_start())
        .ok_or_else(|| "plutil did not return a plist document".to_string())?;
    if !root.starts_with("<dict>") && !root.starts_with("<dict/>") {
        return Err("Info.plist must contain a dictionary".to_string());
    }
    Ok(xml)
}

fn run(command: &mut Command) -> Result<String, String> {
    let output = command.output()
        .map_err(|e| format!("Cannot run {}: {e}", command.get_program().to_string_lossy()))?;
    if !output.status.success() {
        return Err(format!("{} failed ({}): {}{}",
            command.get_program().to_string_lossy(), output.status,
            String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr)));
    }
    String::from_utf8(output.stdout).map_err(|e| format!("Tool output is not UTF-8: {e}"))
}

struct TemporaryDirectory(PathBuf);

impl TemporaryDirectory {
    fn new() -> Result<Self, String> {
        // mktemp creates a private directory atomically, including for parallel
        // builds. Drop cleans it up on both successful and failed merges.
        let path = run(Command::new("/usr/bin/mktemp")
            .args(["-d", "-t", "makepad-info-plist"]))?;
        Ok(Self(PathBuf::from(path.trim_end_matches('\n'))))
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
