use crate::testmap::TestMapBuild;
use makepad_widgets::{Cx, MapViewRef};
use std::ops::{Deref, DerefMut};
use std::path::Path;

pub const PROFILE: super::ProvisioningProfile = super::ProvisioningProfile::Native;

/// Native provisioning keeps the existing first-run download and bake flow.
pub struct MapProvisioner {
    build: TestMapBuild,
    adopted: bool,
}

impl Default for MapProvisioner {
    fn default() -> Self {
        Self {
            build: TestMapBuild::default(),
            adopted: false,
        }
    }
}

pub struct ProvisionerUpdate {
    pub changed: bool,
    pub nav_basename: Option<String>,
}

impl MapProvisioner {
    /// Select production data, an already-baked test map, or start the
    /// established first-run flow. Filesystem policy stays inside this seam.
    pub fn ensure_source(
        &mut self,
        cx: &mut Cx,
        map: &MapViewRef,
        maps_root: &Path,
    ) -> Option<String> {
        self.build.set_maps_root(maps_root);
        if let Some(archive) = crate::testmap::production_archive(maps_root) {
            self.adopt_production_map(cx, map, maps_root, &archive);
            return None;
        }
        if self.build.paths.archive.is_file() {
            self.adopt_existing_test_map(cx, map);
            return None;
        }
        // The DSL has checkout-relative placeholders so it can be previewed,
        // but runtime filesystem policy must never fall back to the cwd.
        map.set_source_paths(cx, "", "", "");
        self.build.offer_if_no_map(false);
        if self.build.is_offered() {
            self.build.start(cx);
        }
        None
    }

    fn adopt_production_map(
        &mut self,
        cx: &mut Cx,
        map: &MapViewRef,
        maps_root: &Path,
        archive: &Path,
    ) {
        if self.adopted {
            return;
        }
        self.adopted = true;
        let archive = archive.to_string_lossy();
        let bridge = maps_root.join("nl-bridge-dz.mbtiles");
        let bridge = bridge
            .is_file()
            .then(|| bridge.to_string_lossy().into_owned())
            .unwrap_or_default();
        map.set_source_paths(cx, &archive, &archive, &bridge);
        let overlays = ["ocean-low.mbtiles", "ocean-high.mbtiles"]
            .into_iter()
            .map(|name| maps_root.join(name))
            .filter(|path| path.is_file())
            .map(|path| path.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(";");
        map.set_overlay_paths(cx, &overlays);
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
        if self.adopted {
            return;
        }
        self.adopted = true;
        let archive = self.build.paths.archive.to_string_lossy().into_owned();
        map.set_source_paths(cx, &archive, &archive, "");
        map.set_overlay_paths(cx, "");
    }

    fn adopt_completed_test_map(&mut self, cx: &mut Cx, map: &MapViewRef) -> Option<String> {
        if self.adopted {
            return None;
        }
        self.adopt_existing_test_map(cx, map);
        map.set_center(cx, crate::AMSTERDAM_CENTER.0, crate::AMSTERDAM_CENTER.1);
        Some(self.build.paths.nav_basename.to_string_lossy().into_owned())
    }
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
