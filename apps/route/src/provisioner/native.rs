use crate::testmap::TestMapBuild;
use makepad_widgets::{Cx, MapViewRef};
use std::ops::{Deref, DerefMut};

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
    pub fn ensure_source(&mut self, cx: &mut Cx, map: &MapViewRef) -> Option<String> {
        if crate::testmap::production_archive_present() {
            return None;
        }
        if self.build.paths.archive.is_file() {
            return self.adopt_test_map(cx, map);
        }
        self.build.offer_if_no_map(false);
        if self.build.is_offered() {
            self.build.start(cx);
        }
        None
    }

    pub fn handle_event(&mut self, cx: &mut Cx, map: &MapViewRef) -> ProvisionerUpdate {
        let changed = self.build.poll();
        let nav_basename = if matches!(self.build.stage, crate::testmap::Stage::Done) {
            self.adopt_test_map(cx, map)
        } else {
            None
        };
        ProvisionerUpdate {
            changed,
            nav_basename,
        }
    }

    fn adopt_test_map(&mut self, cx: &mut Cx, map: &MapViewRef) -> Option<String> {
        if self.adopted {
            return None;
        }
        self.adopted = true;
        let archive = self.build.paths.archive.to_string_lossy().into_owned();
        map.set_source_paths(cx, &archive, &archive, "");
        map.set_overlay_paths(cx, "");
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
