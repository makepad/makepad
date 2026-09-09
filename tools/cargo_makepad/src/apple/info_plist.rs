use super::AppleOs;
use makepad_micro_serde::{DeJson, DeJsonErr, DeJsonState, JsonValue};
use plist::{Dictionary, Value};
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(test)]
mod tests;

pub(super) struct InfoPlist {
    path: PathBuf,
    entries: Dictionary,
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
    let entries = Value::from_file(&path)
        .map_err(|e| format!("Cannot read custom Info.plist {}: {e}", path.display()))?
        .into_dictionary()
        .ok_or_else(|| format!("Custom Info.plist {} must contain a dictionary", path.display()))?;
    Ok(Some(InfoPlist { path, entries }))
}

impl InfoPlist {
    /// Replace whole top-level values, preserving types and unspecified defaults.
    /// Bundle identity and executable stay controlled by the build's CLI inputs:
    /// changing them here would disagree with signing and simulator launch paths.
    pub(super) fn merge(&self, generated: &str) -> Result<String, String> {
        let mut value = Value::from_reader_xml(generated.trim_start().as_bytes())
            .map_err(|e| format!("Cannot parse generated Info.plist: {e}"))?;
        let entries = value.as_dictionary_mut()
            .ok_or_else(|| "Generated Info.plist must contain a dictionary".to_string())?;
        for key in ["CFBundleIdentifier", "CFBundleExecutable"] {
            if let Some(custom) = self.entries.get(key) {
                if Some(custom) != entries.get(key) {
                    return Err(format!(
                        "Custom Info.plist {} cannot change {key}; use the Apple build options and Cargo binary target instead",
                        self.path.display(),
                    ));
                }
            }
        }
        entries.extend(self.entries.clone());
        let mut xml = Vec::new();
        value.to_writer_xml(&mut xml)
            .map_err(|e| format!("Cannot serialize merged Info.plist {}: {e}", self.path.display()))?;
        String::from_utf8(xml).map_err(|e| format!("Cannot encode merged Info.plist: {e}"))
    }
}
