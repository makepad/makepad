use crate::testmap::TestMapBuild;
use makepad_widgets::{Cx, MapViewRef, TileSourceConfig};
use std::fs;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};

pub const PROFILE: super::ProvisioningProfile = super::ProvisioningProfile::Native;
const WORLD_ARCHIVE: &str = "world.mkmap";

/// Native source selection plus the existing test-map state used by the rest
/// of the app's provisioning UI.
pub struct MapProvisioner {
    build: TestMapBuild,
}

impl Default for MapProvisioner {
    fn default() -> Self {
        let _ = fs::remove_file(
            makepad_widgets::makepad_platform::home::makepad_home().join("route/tile-source"),
        );
        Self {
            build: TestMapBuild::default(),
        }
    }
}

pub struct ProvisionerUpdate {
    pub changed: bool,
    pub nav_basename: Option<String>,
}

impl MapProvisioner {
    /// Use a local world archive when the configured maps folder contains
    /// one, otherwise fall back to the hosted range-cached archive.
    pub fn ensure_source(
        &mut self,
        cx: &mut Cx,
        map: &MapViewRef,
        maps_root: &Path,
    ) -> Option<String> {
        self.build.set_maps_root(maps_root);
        self.install_source(cx, map, maps_root);
        None
    }

    fn install_source(
        &mut self,
        cx: &mut Cx,
        map: &MapViewRef,
        maps_root: &Path,
    ) {
        let archive = local_archive(maps_root);
        let bridge = maps_root.join("nl-bridge-dz.mbtiles");
        let bridge = bridge
            .is_file()
            .then(|| bridge.to_string_lossy().into_owned())
            .unwrap_or_default();
        let overlays = ["ocean-low.mbtiles", "ocean-high.mbtiles"]
            .into_iter()
            .map(|name| maps_root.join(name))
            .filter(|path| path.is_file())
            .map(|path| path.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(";");
        let config = if archive.is_file() {
            TileSourceConfig::LocalArchive {
                mbtiles_path: archive.to_string_lossy().into_owned(),
                detail_mbtiles_path: archive.to_string_lossy().into_owned(),
                overlay_mbtiles_paths: overlays,
                bridge_dz_path: bridge,
            }
        } else {
            let mut config = super::demo::hosted_tile_source();
            let TileSourceConfig::HttpArchive {
                overlay_mbtiles_paths,
                bridge_dz_path,
                ..
            } = &mut config
            else {
                unreachable!();
            };
            *overlay_mbtiles_paths = overlays;
            *bridge_dz_path = bridge;
            config
        };
        map.set_source_config(cx, config);
    }

    pub fn handle_event(&mut self, cx: &mut Cx, map: &MapViewRef) -> ProvisionerUpdate {
        let changed = self.build.poll();
        let nav_basename = if matches!(self.build.stage, crate::testmap::Stage::Done) {
            self.adopt_completed_test_map(cx, map)
        } else {
            None
        };
        ProvisionerUpdate {
            changed,
            nav_basename,
        }
    }

    fn adopt_existing_test_map(&mut self, cx: &mut Cx, map: &MapViewRef) {
        let archive = self.build.paths.archive.to_string_lossy().into_owned();
        map.set_source_paths(cx, &archive, &archive, "");
        map.set_overlay_paths(cx, "");
    }

    fn adopt_completed_test_map(&mut self, cx: &mut Cx, map: &MapViewRef) -> Option<String> {
        self.adopt_existing_test_map(cx, map);
        map.set_center(cx, crate::AMSTERDAM_CENTER.0, crate::AMSTERDAM_CENTER.1);
        Some(self.build.paths.nav_basename.to_string_lossy().into_owned())
    }
}

fn local_archive(maps_root: &Path) -> PathBuf {
    maps_root.join(WORLD_ARCHIVE)
}

impl Deref for MapProvisioner {
    type Target = TestMapBuild;

    fn deref(&self) -> &Self::Target {
        &self.build
    }
}

impl DerefMut for MapProvisioner {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.build
    }
}
