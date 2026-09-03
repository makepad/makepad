use crate::testmap::TestMapBuild;
use makepad_widgets::{Cx, MapViewRef, TileSourceConfig};
use std::fs;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};

pub const PROFILE: super::ProvisioningProfile = super::ProvisioningProfile::Native;
const TILE_SOURCE_PREF: &str = "route/tile-source";
const WORLD_ARCHIVE: &str = "world.mkmap";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileSourceChoice {
    Hosted,
    Local,
}

impl TileSourceChoice {
    pub fn from_index(index: usize) -> Self {
        if index == 1 {
            Self::Local
        } else {
            Self::Hosted
        }
    }

    pub fn index(self) -> usize {
        match self {
            Self::Hosted => 0,
            Self::Local => 1,
        }
    }

    fn preference_name(self) -> &'static str {
        match self {
            Self::Hosted => "hosted",
            Self::Local => "local",
        }
    }
}

/// Native source selection plus the existing test-map state used by the rest
/// of the app's provisioning UI.
pub struct MapProvisioner {
    build: TestMapBuild,
    active_choice: TileSourceChoice,
}

impl Default for MapProvisioner {
    fn default() -> Self {
        Self {
            build: TestMapBuild::default(),
            active_choice: TileSourceChoice::Hosted,
        }
    }
}

pub struct ProvisionerUpdate {
    pub changed: bool,
    pub nav_basename: Option<String>,
}

impl MapProvisioner {
    /// Select the saved source, defaulting to the local world only when it is
    /// present. A missing local choice always falls back to hosted tiles.
    pub fn ensure_source(
        &mut self,
        cx: &mut Cx,
        map: &MapViewRef,
        maps_root: &Path,
    ) -> Option<String> {
        self.build.set_maps_root(maps_root);
        let choice = resolve_choice(maps_root);
        self.install_source(cx, map, maps_root, choice);
        None
    }

    pub fn select_source(
        &mut self,
        cx: &mut Cx,
        map: &MapViewRef,
        maps_root: &Path,
        choice: TileSourceChoice,
    ) -> Result<(), String> {
        if choice == TileSourceChoice::Local && !local_archive(maps_root).is_file() {
            return Err(format!(
                "local archive not found: {}",
                local_archive(maps_root).display()
            ));
        }
        save_choice(choice)?;
        self.install_source(cx, map, maps_root, choice);
        Ok(())
    }

    pub fn active_choice(&self) -> TileSourceChoice {
        self.active_choice
    }

    pub fn local_archive_path(maps_root: &Path) -> PathBuf {
        local_archive(maps_root)
    }

    fn install_source(
        &mut self,
        cx: &mut Cx,
        map: &MapViewRef,
        maps_root: &Path,
        choice: TileSourceChoice,
    ) {
        self.active_choice = choice;
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
        let config = match choice {
            TileSourceChoice::Hosted => {
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
            }
            TileSourceChoice::Local => TileSourceConfig::LocalArchive {
                mbtiles_path: archive.to_string_lossy().into_owned(),
                detail_mbtiles_path: archive.to_string_lossy().into_owned(),
                overlay_mbtiles_paths: overlays,
                bridge_dz_path: bridge,
            },
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
        self.active_choice = TileSourceChoice::Local;
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

fn resolve_choice(maps_root: &Path) -> TileSourceChoice {
    let saved = fs::read_to_string(preference_path()).ok();
    resolve_choice_with(saved.as_deref(), local_archive(maps_root).is_file())
}

fn resolve_choice_with(saved: Option<&str>, local_present: bool) -> TileSourceChoice {
    match saved.map(str::trim) {
        Some("hosted") => TileSourceChoice::Hosted,
        Some("local") if local_present => TileSourceChoice::Local,
        _ if local_present => TileSourceChoice::Local,
        _ => TileSourceChoice::Hosted,
    }
}

fn preference_path() -> PathBuf {
    makepad_widgets::makepad_platform::home::makepad_home().join(TILE_SOURCE_PREF)
}

fn save_choice(choice: TileSourceChoice) -> Result<(), String> {
    let path = preference_path();
    let parent = path
        .parent()
        .ok_or_else(|| "tile source preference has no parent".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("create {}: {error}", parent.display()))?;
    let partial = path.with_extension("part");
    fs::write(&partial, choice.preference_name())
        .map_err(|error| format!("write {}: {error}", partial.display()))?;
    fs::rename(&partial, &path)
        .map_err(|error| format!("publish {}: {error}", path.display()))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_source_default_and_saved_choice_obey_local_presence() {
        assert_eq!(resolve_choice_with(None, true), TileSourceChoice::Local);
        assert_eq!(resolve_choice_with(None, false), TileSourceChoice::Hosted);
        assert_eq!(
            resolve_choice_with(Some("hosted"), true),
            TileSourceChoice::Hosted
        );
        assert_eq!(
            resolve_choice_with(Some("local"), true),
            TileSourceChoice::Local
        );
        assert_eq!(
            resolve_choice_with(Some("local"), false),
            TileSourceChoice::Hosted
        );
    }
}
