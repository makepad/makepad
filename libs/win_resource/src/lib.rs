//! Windows executable resources without rc.exe, cvtres or windres.
//!
//! An application icon (from the same `.icns`/`.png`/`.ico` the macOS bundle
//! uses) and optional version information become linker input, so Explorer,
//! the taskbar, pinned shortcuts and Alt-Tab show the icon: a `.res` file for
//! MSVC targets (link.exe and lld-link merge any number of these), or a COFF
//! object with a `.rsrc` section for GNU targets (ld and MinGW lld merge
//! those). The icon group has resource id 1, which is what the platform layer
//! loads for window icons.
//!
//! In a build script:
//! ```ignore
//! makepad_win_resource::link_app_resources(Path::new("resources/icon.icns"), &info, None).unwrap();
//! ```
//! It does nothing unless the target OS is Windows.
pub mod coff;
pub mod icns;
pub mod ico;
pub mod image;
pub mod pe;
pub mod res;
pub mod tree;
pub mod version;

#[cfg(test)]
mod tests;

pub use coff::{read_object, write_object, Machine};
pub use ico::{icon_images, load_source, IconImage};
pub use pe::read_pe_resources;
pub use res::{read_res, write_res};
pub use tree::{ResId, Resource, RT_GROUP_ICON, RT_ICON, RT_MANIFEST, RT_VERSION};
pub use version::VersionInfo;

use std::path::{Path, PathBuf};

/// Resource id of the application icon group. Windows uses the first icon
/// group for the executable's icon; the platform layer loads id 1.
pub const APP_ICON_ID: u16 = 1;

/// `RT_ICON` images (ids 1..) and an `RT_GROUP_ICON` (id 1) from an icon
/// source, plus `RT_VERSION` when given.
pub fn app_resources(icon_source: Option<&[u8]>, version: Option<&VersionInfo>) -> Result<Vec<Resource>, String> {
    let mut resources = Vec::new();
    if let Some(source) = icon_source {
        let images = icon_images(&load_source(source)?)?;
        resources.push(Resource::new(RT_GROUP_ICON, APP_ICON_ID, ico::group_icon(&images, 1)));
        for (i, image) in images.into_iter().enumerate() {
            resources.push(Resource::new(RT_ICON, 1 + i as u16, image.data));
        }
    }
    if let Some(info) = version {
        resources.push(Resource::new(RT_VERSION, 1, version::version_resource(info)));
    }
    Ok(resources)
}

/// How resources reach the linker for a target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkFormat {
    /// A `.res` file, for link.exe and lld-link (MSVC targets).
    Res,
    /// A COFF object with `.rsrc$01`/`.rsrc$02`, for GNU targets.
    Coff(Machine),
}

impl LinkFormat {
    /// From `CARGO_CFG_TARGET_{OS,ENV,ARCH}`; `None` when the target is not Windows.
    pub fn for_target(os: &str, env: &str, arch: &str) -> Result<Option<LinkFormat>, String> {
        if os != "windows" {
            return Ok(None);
        }
        if env == "msvc" {
            return Ok(Some(LinkFormat::Res));
        }
        let machine = Machine::from_target_arch(arch).ok_or_else(|| format!("no resource object format for target arch {arch}"))?;
        Ok(Some(LinkFormat::Coff(machine)))
    }
    pub fn extension(self) -> &'static str {
        match self {
            LinkFormat::Res => "res",
            LinkFormat::Coff(_) => "o",
        }
    }
    pub fn write(self, resources: &[Resource]) -> Result<Vec<u8>, String> {
        match self {
            LinkFormat::Res => Ok(write_res(resources)),
            LinkFormat::Coff(machine) => write_object(machine, resources),
        }
    }
}

/// Linker input holding the application's icon and version resources.
pub fn app_link_input(format: LinkFormat, icon_source: Option<&[u8]>, version: Option<&VersionInfo>) -> Result<Vec<u8>, String> {
    format.write(&app_resources(icon_source, version)?)
}

/// Build-script helper: for Windows targets write the linker input to
/// `OUT_DIR` and link it into the package's binaries (or only `bin`).
/// Returns its path, or `None` for other targets.
pub fn link_app_resources(icon: &Path, version: &VersionInfo, bin: Option<&str>) -> Result<Option<PathBuf>, String> {
    link_app_resources_with(icon, version, None, bin)
}

/// An application manifest that runs the program as the invoking user
/// (no elevation, no installer detection) on Windows 10 and 11, and says
/// nothing else: DPI awareness and the like stay with the program.
pub const AS_INVOKER_MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
</assembly>
"#;

/// [`link_app_resources`] with an application manifest (resource id 1).
pub fn link_app_resources_with(icon: &Path, version: &VersionInfo, manifest: Option<&str>, bin: Option<&str>) -> Result<Option<PathBuf>, String> {
    println!("cargo:rerun-if-changed={}", icon.display());
    let var = |name: &str| std::env::var(name).map_err(|_| format!("{name} is not set; call this from a build script"));
    let Some(format) = LinkFormat::for_target(&var("CARGO_CFG_TARGET_OS")?, &var("CARGO_CFG_TARGET_ENV").unwrap_or_default(), &var("CARGO_CFG_TARGET_ARCH")?)? else {
        return Ok(None);
    };
    let source = std::fs::read(icon).map_err(|e| format!("read icon {}: {e}", icon.display()))?;
    let mut resources = app_resources(Some(&source), Some(version)).map_err(|e| format!("icon {}: {e}", icon.display()))?;
    if let Some(manifest) = manifest {
        resources.push(Resource::new(RT_MANIFEST, 1, manifest.as_bytes().to_vec()));
    }
    let input = format.write(&resources)?;
    let path = PathBuf::from(var("OUT_DIR")?).join(format!("makepad-app-resources.{}", format.extension()));
    std::fs::write(&path, input).map_err(|e| format!("write {}: {e}", path.display()))?;
    match bin {
        Some(bin) => println!("cargo:rustc-link-arg-bin={bin}={}", path.display()),
        None => println!("cargo:rustc-link-arg-bins={}", path.display()),
    }
    Ok(Some(path))
}
