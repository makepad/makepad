use crate::testmap::TestMapBuild;
use crate::overlays::{overlay_source, OverlaySelection, OVERLAY_LAYERS};
use makepad_widgets::{Cx, MapViewRef, OverlaySource, TileSourceConfig};
use std::fs;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};

pub const PROFILE: super::ProvisioningProfile = super::ProvisioningProfile::Native;
const WORLD_ARCHIVE: &str = "world.mkmap";
const OCEAN_MBTILES: [&str; 2] = ["ocean-low.mbtiles", "ocean-high.mbtiles"];

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
        let config = if archive.is_file() {
            TileSourceConfig::LocalArchive {
                mbtiles_path: archive.to_string_lossy().into_owned(),
                detail_mbtiles_path: archive.to_string_lossy().into_owned(),
                bridge_dz_path: bridge,
            }
        } else {
            let mut config = super::demo::hosted_tile_source();
            let TileSourceConfig::HttpArchive {
                bridge_dz_path,
                ..
            } = &mut config
            else {
                unreachable!();
            };
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

    pub fn overlay_sources(
        &self,
        selection: &OverlaySelection,
        maps_root: &Path,
    ) -> Vec<OverlaySource> {
        let mut sources = OCEAN_MBTILES
            .into_iter()
            .map(|name| maps_root.join(name))
            .filter(|path| path.is_file())
            .map(|path| {
                let name = path
                    .file_stem()
                    .and_then(|name| name.to_str())
                    .unwrap_or("ocean");
                OverlaySource::new(
                    name,
                    TileSourceConfig::local_archive(path.to_string_lossy().into_owned()),
                )
            })
            .collect::<Vec<_>>();
        let available = OVERLAY_LAYERS
            .iter()
            .zip(super::demo::HOSTED_CONFIG.overlays)
            .map(|(layer, url)| {
                let source = Path::new(layer.local_mbtiles);
                let source = if source.is_file() {
                    TileSourceConfig::local_archive(layer.local_mbtiles)
                } else {
                    TileSourceConfig::http_archive(url)
                };
                overlay_source(*layer, source)
            })
            .collect::<Vec<_>>();
        sources.extend(selection.enabled_sources(&available));
        sources
    }

    fn adopt_existing_test_map(&mut self, cx: &mut Cx, map: &MapViewRef) {
        let archive = self.build.paths.archive.to_string_lossy().into_owned();
        map.set_source_paths(cx, &archive, &archive, "");
        map.set_overlays(cx, Vec::new());
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
