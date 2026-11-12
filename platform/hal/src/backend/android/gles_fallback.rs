//! A note in the app's cache dir that this phone's Vulkan driver refused our pipelines, so later
//! launches draw with GLES right away, until a system update, an app update or a new driver.

use super::shell::AndroidBackend;
use std::path::PathBuf;

fn note() -> Option<PathBuf> {
    let cache = AndroidBackend::params().cache_path;
    (!cache.is_empty()).then(|| PathBuf::from(cache).join("makepad-hal").join("vulkan-refused"))
}

/// The system's build and the app's, since a note only counts for the builds that wrote it.
fn builds() -> String {
    let fingerprint = super::xr::system_property("ro.build.fingerprint").unwrap_or_default();
    format!("{fingerprint}\n{}\n", AndroidBackend::params().app_build)
}

fn identity(driver: [u32; 3]) -> String {
    format!("{}{:x}-{:x}-{:x}\n", builds(), driver[0], driver[1], driver[2])
}

/// Notes that `driver` refused a pipeline (`why`) on this system and app build.
pub(super) fn remember(driver: [u32; 3], why: &str) {
    let Some(path) = note() else { return };
    let written = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::write(&path, format!("{}{why}\n", identity(driver))));
    if let Err(e) = written { eprintln!("[hal-android] {}: {e}", path.display()); }
}

/// Whether this Vulkan driver refused our pipelines in an earlier launch of the same system and
/// app build. A note about other builds or another driver is deleted.
pub(super) fn remembered() -> bool {
    let Some(path) = note() else { return false };
    let Ok(text) = std::fs::read_to_string(&path) else { return false };
    // the builds are checked first, since reading the driver's version loads it
    let same = text.starts_with(&builds())
        && crate::backend::vulkan::first_gpu_driver().is_some_and(|driver| text.starts_with(&identity(driver)));
    if !same { let _ = std::fs::remove_file(&path); }
    same
}
