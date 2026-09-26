//! The one icon file per app that both the macOS bundle (AppIcon) and the
//! Windows executable (icon resource) are made from.
use crate::catalog::Release;
use std::{borrow::Cow, fs, io, path::{Path, PathBuf}};

pub enum IconSource {
    /// The app's `resources/icon.icns`, else `resources/icon_1024.png`, else
    /// the icon its package declares; Scope's is in the Makepad tree it
    /// builds from (`tools/makepad_builder/resources/scope.icns`).
    File(PathBuf),
}

impl IconSource {
    pub fn is_png(&self) -> bool {
        matches!(self, IconSource::File(path) if path.extension().is_some_and(|e| e == "png"))
    }
    pub fn read(&self) -> io::Result<Cow<'static, [u8]>> {
        match self {
            IconSource::File(path) => fs::read(path).map(Cow::Owned),
        }
    }
}

pub fn source(root: &Path, release: &Release) -> Option<IconSource> {
    if release.id == "scope" {
        let makepad = release.repositories.iter().find(|repo| repo.name == "makepad")?;
        let icon = release.directory(root).join(&makepad.path).join("tools/makepad_builder/resources/scope.icns");
        return icon.is_file().then_some(IconSource::File(icon));
    }
    let workspace = release.source(root);
    let resources = workspace.join("resources");
    ["icon.icns", "icon_1024.png"].into_iter().map(|name| resources.join(name)).find(|path| path.is_file())
        .or_else(|| declared_icon(&workspace, &release.package))
        .map(IconSource::File)
}

/// The icon the app's package declares for desktop bundles, as `cargo
/// makepad` reads it: `icon = "…"` under `[package.metadata.makepad.desktop]`
/// in the Cargo.toml whose package is `package`, relative to that file. The
/// package is looked for in the workspace, a few folders deep.
fn declared_icon(workspace: &Path, package: &str) -> Option<PathBuf> {
    let mut folders = vec![(workspace.to_path_buf(), 0)];
    while let Some((folder, depth)) = folders.pop() {
        let manifest = folder.join("Cargo.toml");
        if let Ok(text) = fs::read_to_string(&manifest) {
            if manifest_value(&text, "package", "name").as_deref() == Some(package) {
                return manifest_value(&text, "package.metadata.makepad.desktop", "icon")
                    .map(|icon| folder.join(icon))
                    .filter(|path| path.is_file());
            }
        }
        if depth < 3 {
            for entry in fs::read_dir(&folder).into_iter().flatten().flatten() {
                let name = entry.file_name();
                if entry.file_type().is_ok_and(|t| t.is_dir()) && !matches!(name.to_str(), Some("target" | ".git" | "resources" | "src")) {
                    folders.push((entry.path(), depth + 1));
                }
            }
        }
    }
    None
}

/// A plain `key = "value"` in the TOML table `[table]`.
fn manifest_value(text: &str, table: &str, key: &str) -> Option<String> {
    let mut inside = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            inside = line == format!("[{table}]");
        } else if inside {
            if let Some((k, v)) = line.split_once('=') {
                if k.trim() == key {
                    let v = v.trim();
                    return v.strip_prefix('"').and_then(|v| v.split('"').next()).map(str::to_owned);
                }
            }
        }
    }
    None
}

/// The Windows resources for a built app: its icon (when it has one) and
/// version information naming it, as a `.res` file for the linker. The file
/// name carries a content hash, so a changed icon changes the final link's
/// command line and Cargo relinks.
#[cfg(windows)]
pub fn windows_resources(root: &Path, build: &Path, release: &Release) -> Result<PathBuf, String> {
    use makepad_win_resource::{app_link_input, LinkFormat, VersionInfo};
    let icon = match source(root, release) {
        Some(source) => Some(source.read().map_err(|e| format!("Read the {} icon: {e}", release.title))?),
        None => None,
    };
    let info = VersionInfo {
        file_description: release.title.clone(),
        product_name: release.title.clone(),
        internal_name: release.binary.clone(),
        original_filename: format!("{}.exe", release.binary),
        version_text: release.release.clone(),
        version: [1, 0, 0, 0],
        ..Default::default()
    };
    let res = app_link_input(LinkFormat::Res, icon.as_deref(), Some(&info)).map_err(|e| format!("{} icon: {e}", release.title))?;
    let dir = build.join("app-resources");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{}-{}.res", release.binary, &crate::sha256::sha256_hex(&res)[..16]));
    if !path.is_file() {
        let next = path.with_extension("next");
        fs::write(&next, &res).map_err(|e| e.to_string())?;
        fs::rename(&next, &path).map_err(|e| e.to_string())?;
    }
    Ok(path)
}
