//! Where this instance keeps its map data, resolved ONCE when the view
//! starts and held on the view (`RouteView::map_root`) — never a static,
//! never the process cwd.
//!
//! An executable running out of a checkout uses that checkout's
//! `local/maps`; a copied or installed executable — and a host with no
//! checkout around it, such as the all-in-one on a phone — uses Makepad's
//! per-user home under `route/maps`. A developer override in the home's
//! `route/maps-root` file wins over both.
//!
//! The overlay caches (radar, wind, chargers) and the drive history are
//! siblings of the maps root, so a checkout keeps today's `local/overlays`
//! and `local/route_history`, and a home layout stays under `route/`.

use std::fs;
use std::path::{Path, PathBuf};

const MAPS_ROOT_PREF: &str = "route/maps-root";

/// The instance's map data layout, derived from one root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoutePaths {
    /// The archives, nav artifacts and the test map.
    pub maps: PathBuf,
    /// Overlay caches: `radar/`, `wind/`, `nl-chargers.mbtiles`.
    pub overlays: PathBuf,
    /// The append-only drive records.
    pub history: PathBuf,
}

impl RoutePaths {
    pub fn from_maps_root(maps: PathBuf) -> RoutePaths {
        let parent = maps.parent().map(Path::to_path_buf).unwrap_or_default();
        RoutePaths {
            overlays: parent.join("overlays"),
            history: parent.join("route_history"),
            maps,
        }
    }

    pub fn radar_cache(&self) -> PathBuf {
        self.overlays.join("radar")
    }

    pub fn wind_cache(&self) -> PathBuf {
        self.overlays.join("wind")
    }

    pub fn chargers(&self) -> PathBuf {
        self.overlays.join("nl-chargers.mbtiles")
    }
}

/// Resolve the maps root for this process: the override file, the
/// checkout the executable runs from, else the makepad home.
pub fn resolve_maps_root() -> PathBuf {
    let home = makepad_widgets::makepad_platform::home::makepad_home();
    let explicit = fs::read_to_string(home.join(MAPS_ROOT_PREF))
        .ok()
        .map(|value| PathBuf::from(value.trim()))
        .filter(|path| !path.as_os_str().is_empty());
    let executable = std::env::current_exe().unwrap_or_default();
    // The checkout this binary was BUILT in, by its compiled-in manifest
    // path: a simulator app runs on the machine that built it and reads
    // that checkout's maps; on a device or another machine the path is
    // simply absent and the walk from the executable decides.
    let built_in = Path::new(env!("CARGO_MANIFEST_DIR")).parent().and_then(Path::parent).map(Path::to_path_buf);
    let built_in = built_in.filter(|repo| {
        fs::read_to_string(repo.join("Cargo.toml"))
            .is_ok_and(|text| text.lines().any(|l| { let l = l.trim(); l == "[workspace]" || l.starts_with("workspace.members") }))
            && repo.join("local").join("maps").is_dir()
    });
    if let (None, Some(repo)) = (explicit.as_deref(), built_in) {
        return repo.join("local").join("maps");
    }
    resolve_maps_root_from(&executable, &home, explicit.as_deref())
}

fn resolve_maps_root_from(executable: &Path, home: &Path, explicit: Option<&Path>) -> PathBuf {
    resolve_maps_root_with(executable, home, explicit, |manifest| {
        fs::read_to_string(manifest).is_ok_and(|text| {
            text.lines().any(|line| {
                let line = line.trim();
                line == "[workspace]" || line.starts_with("workspace.members")
            })
        })
    })
}

fn resolve_maps_root_with(
    executable: &Path,
    home: &Path,
    explicit: Option<&Path>,
    mut is_workspace_manifest: impl FnMut(&Path) -> bool,
) -> PathBuf {
    if let Some(explicit) = explicit {
        return explicit.to_path_buf();
    }
    let mut directory = executable.parent();
    while let Some(candidate) = directory {
        if is_workspace_manifest(&candidate.join("Cargo.toml")) {
            return candidate.join("local/maps");
        }
        directory = candidate.parent();
    }
    home.join("route/maps")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_root_uses_checkout_found_from_executable() {
        let executable = Path::new("/checkout/target/release/route");
        let root = resolve_maps_root_with(executable, Path::new("/home/.makepad"), None, |path| {
            path == Path::new("/checkout/Cargo.toml")
        });
        assert_eq!(root, PathBuf::from("/checkout/local/maps"));
    }

    #[test]
    fn maps_root_for_a_binary_copy_uses_makepad_home() {
        let root = resolve_maps_root_with(
            Path::new("/Applications/Makepad/route"),
            Path::new("/home/.makepad"),
            None,
            |_| false,
        );
        assert_eq!(root, PathBuf::from("/home/.makepad/route/maps"));
    }

    #[test]
    fn explicit_maps_root_wins_over_checkout() {
        let root = resolve_maps_root_with(
            Path::new("/checkout/target/release/route"),
            Path::new("/home/.makepad"),
            Some(Path::new("/data/maps")),
            |_| true,
        );
        assert_eq!(root, PathBuf::from("/data/maps"));
    }

    #[test]
    fn caches_and_history_sit_beside_the_maps() {
        let paths = RoutePaths::from_maps_root(PathBuf::from("/checkout/local/maps"));
        assert_eq!(paths.overlays, PathBuf::from("/checkout/local/overlays"));
        assert_eq!(paths.history, PathBuf::from("/checkout/local/route_history"));
        assert_eq!(paths.radar_cache(), PathBuf::from("/checkout/local/overlays/radar"));
        assert_eq!(paths.chargers(), PathBuf::from("/checkout/local/overlays/nl-chargers.mbtiles"));
        let home = RoutePaths::from_maps_root(PathBuf::from("/home/.makepad/route/maps"));
        assert_eq!(home.wind_cache(), PathBuf::from("/home/.makepad/route/overlays/wind"));
        assert_eq!(home.history, PathBuf::from("/home/.makepad/route/route_history"));
    }
}
