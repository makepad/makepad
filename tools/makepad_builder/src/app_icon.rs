//! The one icon file per app that both the macOS bundle (AppIcon) and the
//! Windows executable (icon resource) are made from.
use crate::catalog::Release;
use std::{borrow::Cow, fs, io, path::{Path, PathBuf}};

/// Scope's icon lives with the Builder; makepad-builder.exe carries it too
/// (see build.rs).
pub const SCOPE_ICNS: &[u8] = include_bytes!("../resources/scope.icns");

pub enum IconSource {
    Scope,
    /// The app's `resources/icon.icns`, else `resources/icon_1024.png`.
    File(PathBuf),
}

impl IconSource {
    pub fn is_png(&self) -> bool {
        matches!(self, IconSource::File(path) if path.extension().is_some_and(|e| e == "png"))
    }
    pub fn read(&self) -> io::Result<Cow<'static, [u8]>> {
        match self {
            IconSource::Scope => Ok(Cow::Borrowed(SCOPE_ICNS)),
            IconSource::File(path) => fs::read(path).map(Cow::Owned),
        }
    }
}

pub fn source(root: &Path, release: &Release) -> Option<IconSource> {
    if release.id == "scope" {
        return Some(IconSource::Scope);
    }
    let resources = release.source(root).join("resources");
    ["icon.icns", "icon_1024.png"].into_iter().map(|name| resources.join(name)).find(|path| path.is_file()).map(IconSource::File)
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
