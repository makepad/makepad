use crate::overlays::{
    overlay_source, OverlaySelection, OCEAN_OVERLAY_LAYERS, OVERLAY_LAYERS,
};
#[cfg(feature = "bake")]
use crate::testmap::{Stage, TestMapBuild};
use makepad_widgets::{Cx, MapViewRef, NetworkResponse, OverlaySource, TileSourceConfig};
use std::fs;
use std::path::{Path, PathBuf};

pub const PROFILE: super::ProvisioningProfile = super::ProvisioningProfile::Native;
const WORLD_ARCHIVE: &str = "world.mkmap";

/// Native source selection: a local world archive when the maps root has
/// one, else the hosted range-cached archive — and, with `bake`, the
/// first-run test-map build the popup card drives.
pub struct MapProvisioner {
    maps_root: PathBuf,
    #[cfg(feature = "bake")]
    build: Option<TestMapBuild>,
}

impl Default for MapProvisioner {
    fn default() -> Self {
        let _ = fs::remove_file(
            makepad_widgets::makepad_platform::home::makepad_home().join("route/tile-source"),
        );
        Self {
            maps_root: PathBuf::new(),
            #[cfg(feature = "bake")]
            build: None,
        }
    }
}

pub struct ProvisionerUpdate {
    pub changed: bool,
    pub nav_basename: Option<String>,
}

/// What the first-run card shows while a build is offered, running,
/// finished or failed.
#[derive(Clone, Debug, PartialEq)]
pub struct TestMapCard {
    pub headline: String,
    pub status: String,
    pub log: String,
    /// Whole-recipe progress, 0..1.
    pub fraction: f32,
    /// The start button does something (offered, or failed and retryable).
    pub can_start: bool,
    pub start_label: &'static str,
    pub dismiss_label: &'static str,
    /// The bake owns the machine for a minute and has no cancel; the
    /// card stays put rather than offering a button that does nothing.
    pub dismissable: bool,
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
        self.maps_root = maps_root.to_path_buf();
        #[cfg(feature = "bake")]
        {
            self.build = Some(TestMapBuild::new(maps_root));
        }
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

    /// Poll the bake. A finished one is adopted into the map and its nav
    /// basename returned for loading.
    pub fn handle_event(&mut self, cx: &mut Cx, map: &MapViewRef) -> ProvisionerUpdate {
        #[cfg(feature = "bake")]
        if let Some(build) = &mut self.build {
            let changed = build.poll();
            let nav_basename = if matches!(build.stage, Stage::Done) {
                self.adopt_completed_test_map(cx, map)
            } else {
                None
            };
            return ProvisionerUpdate {
                changed,
                nav_basename,
            };
        }
        let _ = (cx, map);
        ProvisionerUpdate {
            changed: false,
            nav_basename: None,
        }
    }

    /// The extract download rides the platform's HTTP stack. True when
    /// the response was the build's and the card should refresh.
    pub fn handle_network(&mut self, cx: &mut Cx, response: &NetworkResponse) -> bool {
        #[cfg(feature = "bake")]
        if let Some(build) = &mut self.build {
            return match response {
                NetworkResponse::HttpProgress {
                    request_id,
                    progress,
                } => build.handle_http_progress(*request_id, progress),
                NetworkResponse::HttpResponse {
                    request_id,
                    response,
                } => build.handle_http_response(cx, *request_id, response),
                NetworkResponse::HttpError { request_id, error } => {
                    build.handle_http_error(*request_id, &error.message)
                }
                _ => false,
            };
        }
        let _ = (cx, response);
        false
    }

    /// The card, while there is something to show.
    pub fn card(&self) -> Option<TestMapCard> {
        #[cfg(feature = "bake")]
        if let Some(build) = &self.build {
            if !build.is_active() {
                return None;
            }
            let done = matches!(build.stage, Stage::Done);
            return Some(TestMapCard {
                headline: build.headline.clone(),
                status: build.status_line(),
                log: build.log.join("\n"),
                fraction: build.fraction,
                can_start: build.can_start() && !build.is_running() && !done,
                start_label: if matches!(build.stage, Stage::Failed(_)) {
                    "Try again"
                } else {
                    "Build test map"
                },
                dismiss_label: match build.stage {
                    Stage::Fetching { .. } => "Cancel",
                    Stage::Done => "Start driving",
                    _ => "Not now",
                },
                dismissable: !matches!(build.stage, Stage::Baking),
            });
        }
        None
    }

    /// The card's first button.
    pub fn start(&mut self, cx: &mut Cx) {
        #[cfg(feature = "bake")]
        if let Some(build) = &mut self.build {
            build.start(cx);
        }
        let _ = cx;
    }

    /// The card's second button.
    pub fn dismiss(&mut self) {
        #[cfg(feature = "bake")]
        if let Some(build) = &mut self.build {
            build.dismiss();
        }
    }

    pub fn overlay_sources(
        &self,
        selection: &OverlaySelection,
        maps_root: &Path,
    ) -> Vec<OverlaySource> {
        let (ocean_urls, selectable_urls) = super::demo::HOSTED_CONFIG
            .overlays
            .split_at(OCEAN_OVERLAY_LAYERS.len());
        let mut sources = OCEAN_OVERLAY_LAYERS
            .iter()
            .zip(ocean_urls.iter().copied())
            .map(|(layer, url)| {
                let path = maps_root.join(layer.local_mbtiles);
                let source = if path.is_file() {
                    TileSourceConfig::local_archive(path.to_string_lossy().into_owned())
                } else {
                    TileSourceConfig::http_archive(url)
                };
                overlay_source(*layer, source)
            })
            .collect::<Vec<_>>();
        let available = OVERLAY_LAYERS
            .iter()
            .zip(selectable_urls.iter().copied())
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

    #[cfg(feature = "bake")]
    fn adopt_completed_test_map(&mut self, cx: &mut Cx, map: &MapViewRef) -> Option<String> {
        let build = self.build.as_ref()?;
        let archive = build.paths.archive.to_string_lossy().into_owned();
        map.set_source_paths(cx, &archive, &archive, "");
        map.set_overlays(cx, Vec::new());
        map.set_center(cx, crate::AMSTERDAM_CENTER.0, crate::AMSTERDAM_CENTER.1);
        Some(build.paths.nav_basename.to_string_lossy().into_owned())
    }
}

fn local_archive(maps_root: &Path) -> PathBuf {
    maps_root.join(WORLD_ARCHIVE)
}
