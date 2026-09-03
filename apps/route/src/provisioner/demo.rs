use crate::overlays::{OCEAN_OVERLAY_LAYERS, OVERLAY_LAYERS};
#[cfg(any(feature = "demo", test))]
use crate::overlays::{overlay_source, OverlaySelection};
use makepad_widgets::TileSourceConfig;
#[cfg(any(feature = "demo", test))]
use makepad_widgets::OverlaySource;
#[cfg(feature = "demo")]
use makepad_widgets::{Cx, MapViewRef};

#[cfg(any(feature = "demo", test))]
pub const PROFILE: super::ProvisioningProfile = super::ProvisioningProfile::Demo;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostedConfig {
    pub tiles: &'static str,
    pub overlays: [&'static str; OCEAN_OVERLAY_LAYERS.len() + OVERLAY_LAYERS.len()],
    pub api: &'static str,
}

pub const HOSTED_CONFIG: HostedConfig = HostedConfig {
    tiles: "https://makepad.nl/maps/world-20260903.mkmap",
    overlays: [
        "https://makepad.nl/maps/overlays/ocean-low-20260903.mkmap/",
        "https://makepad.nl/maps/overlays/ocean-high-20260904.mkmap/",
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

#[cfg(any(feature = "demo", test))]
fn hosted_overlay_sources(selection: &OverlaySelection) -> Vec<OverlaySource> {
    let (ocean_urls, selectable_urls) = HOSTED_CONFIG
        .overlays
        .split_at(OCEAN_OVERLAY_LAYERS.len());
    let mut sources = OCEAN_OVERLAY_LAYERS
        .iter()
        .zip(ocean_urls.iter().copied())
        .map(|(layer, url)| overlay_source(*layer, TileSourceConfig::http_archive(url)))
        .collect::<Vec<_>>();
    let available = OVERLAY_LAYERS
        .iter()
        .zip(selectable_urls.iter().copied())
        .map(|(layer, url)| overlay_source(*layer, TileSourceConfig::http_archive(url)))
        .collect::<Vec<_>>();
    sources.extend(selection.enabled_sources(&available));
    sources
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
        hosted_overlay_sources(selection)
    }

    pub fn api_url(&self) -> &'static str {
        HOSTED_CONFIG.api
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_overlay_sources_include_both_ocean_archives() {
        let sources = hosted_overlay_sources(&OverlaySelection::default());
        assert_eq!(sources.len(), OCEAN_OVERLAY_LAYERS.len());
        assert_eq!(sources[0].name, "ocean");
        assert_eq!(sources[1].name, "ocean");
        assert_eq!(
            sources[0].source,
            TileSourceConfig::http_archive(HOSTED_CONFIG.overlays[0]),
        );
        assert_eq!(
            sources[1].source,
            TileSourceConfig::http_archive(HOSTED_CONFIG.overlays[1]),
        );
    }
}
