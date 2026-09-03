use makepad_widgets::*;

pub const OVERLAY_COUNT: usize = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OverlayLayer {
    pub name: &'static str,
    pub local_mbtiles: &'static str,
    pub option: Option<&'static str>,
}

/// One ordered layer table drives native tools, both checkbox controllers,
/// and the provisioners' source tables.
pub const OVERLAY_LAYERS: [OverlayLayer; OVERLAY_COUNT] = [
    OverlayLayer {
        name: "chargers",
        local_mbtiles: "local/overlays/nl-chargers.mbtiles",
        option: Some("fast"),
    },
    OverlayLayer {
        name: "transit",
        local_mbtiles: "local/overlays/nl-transit.mbtiles",
        option: None,
    },
    OverlayLayer {
        name: "nature",
        local_mbtiles: "local/overlays/nl-nature.mbtiles",
        option: None,
    },
    OverlayLayer {
        name: "districts",
        local_mbtiles: "local/overlays/nl-wijkbuurt.mbtiles",
        option: None,
    },
    OverlayLayer {
        name: "buildings_age",
        local_mbtiles: "local/overlays/nl-buildings-age.mbtiles",
        option: None,
    },
    OverlayLayer {
        name: "demographics",
        local_mbtiles: "local/overlays/nl-demographics.mbtiles",
        option: None,
    },
];

#[derive(Default)]
pub struct OverlaySelection {
    pub on: [bool; OVERLAY_COUNT],
}

impl OverlaySelection {
    pub fn set_named(&mut self, name: &str, on: bool) -> Option<&'static str> {
        let key = name.trim().to_ascii_lowercase();
        for (index, layer) in OVERLAY_LAYERS.iter().enumerate() {
            if layer.name == key
                || (key == "wijkbuurt" && layer.name == "districts")
                || (key == "buildings-age" && layer.name == "buildings_age")
            {
                self.on[index] = on;
                return Some(layer.name);
            }
        }
        None
    }

    pub fn enabled_sources(&self, available: &[OverlaySource]) -> Vec<OverlaySource> {
        available
            .iter()
            .zip(self.on.iter())
            .filter(|(_, on)| **on)
            .map(|(source, _)| source.clone())
            .collect()
    }

    pub fn enabled_names(&self) -> Vec<&'static str> {
        OVERLAY_LAYERS
            .iter()
            .zip(self.on.iter())
            .filter(|(_, on)| **on)
            .map(|(layer, _)| layer.name)
            .collect()
    }
}

pub fn overlay_source(layer: OverlayLayer, source: TileSourceConfig) -> OverlaySource {
    OverlaySource::with_option(layer.name, source, layer.option)
}

fn checkbox_ids() -> [&'static [LiveId]; OVERLAY_COUNT] {
    [
        ids!(layer_chargers),
        ids!(layer_transit),
        ids!(layer_nature),
        ids!(layer_districts),
        ids!(layer_buildings),
        ids!(layer_demographics),
    ]
}

pub fn sync_checkboxes(cx: &mut Cx, ui: &WidgetRef, selection: &OverlaySelection) {
    for (id, on) in checkbox_ids().into_iter().zip(selection.on) {
        ui.check_box(cx, id).set_active(cx, on, Animate::No);
    }
}

pub fn handle_checkboxes(
    cx: &mut Cx,
    ui: &WidgetRef,
    actions: &Actions,
    selection: &mut OverlaySelection,
) -> bool {
    let mut changed = false;
    for (index, id) in checkbox_ids().into_iter().enumerate() {
        if let Some(on) = ui.check_box(cx, id).changed(actions) {
            selection.on[index] = on;
            changed = true;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_aliases_share_the_ordered_layer_table() {
        let mut selection = OverlaySelection::default();
        assert_eq!(selection.set_named("wijkbuurt", true), Some("districts"));
        assert_eq!(selection.set_named("buildings-age", true), Some("buildings_age"));
        assert!(selection.on[3]);
        assert!(selection.on[4]);
    }

    #[test]
    fn charger_filter_stays_on_its_source() {
        let source = overlay_source(
            OVERLAY_LAYERS[0],
            TileSourceConfig::http_archive("https://makepad.nl/maps/chargers.mkmap/"),
        );
        assert_eq!(source.name, "chargers");
        let TileSourceConfig::HttpArchive { root_url, .. } = source.source else {
            panic!("hosted overlay changed source kind");
        };
        assert_eq!(root_url, "https://makepad.nl/maps/chargers.mkmap/?fast");
    }
}
