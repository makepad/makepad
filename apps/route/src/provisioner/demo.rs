use crate::overlays::OVERLAY_LAYERS;
#[cfg(feature = "demo")]
use crate::overlays::{overlay_source, OverlaySelection};
use makepad_widgets::TileSourceConfig;
#[cfg(feature = "demo")]
use makepad_widgets::{Cx, MapViewRef, OverlaySource};

#[cfg(any(feature = "demo", test))]
pub const PROFILE: super::ProvisioningProfile = super::ProvisioningProfile::Demo;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostedConfig {
    pub tiles: &'static str,
    pub overlays: [&'static str; OVERLAY_LAYERS.len()],
    pub api: &'static str,
}

pub const HOSTED_CONFIG: HostedConfig = HostedConfig {
    tiles: "https://makepad.nl/maps/world-20260903.mkmap",
    overlays: [
        "https://makepad.nl/maps/overlays/chargers-20260903.mkmap/",
        "https://makepad.nl/maps/overlays/transit-20260903.mkmap/",
        "https://makepad.nl/maps/overlays/nature-20260903.mkmap/",
        "https://makepad.nl/maps/overlays/wijkbuurt-20260903.mkmap/",
        "https://makepad.nl/maps/overlays/buildings-age-20260903.mkmap/",
        "https://makepad.nl/maps/overlays/demographics-20260903.mkmap/",
    ],
    api: "https://makepad.nl/api",
};

pub fn hosted_tile_source() -> TileSourceConfig {
    TileSourceConfig::http_archive(HOSTED_CONFIG.tiles)
}

/// Hosted demo provisioning has no modal, filesystem probes, or bake state.
#[derive(Default)]
#[cfg(feature = "demo")]
pub struct MapProvisioner {
    installed: bool,
}

#[cfg(feature = "demo")]
impl MapProvisioner {
    pub fn ensure_source(&mut self, cx: &mut Cx, map: &MapViewRef) {
        if self.installed {
            return;
        }
        self.installed = true;
        map.set_source_config(cx, hosted_tile_source());
    }

    pub fn handle_event(&mut self) {}

    pub fn overlay_sources(&self, selection: &OverlaySelection) -> Vec<OverlaySource> {
        let available = OVERLAY_LAYERS
            .iter()
            .zip(HOSTED_CONFIG.overlays)
            .map(|(layer, url)| overlay_source(*layer, TileSourceConfig::http_archive(url)))
            .collect::<Vec<_>>();
        selection.enabled_sources(&available)
    }

    pub fn api_url(&self) -> &'static str {
        HOSTED_CONFIG.api
    }
}
