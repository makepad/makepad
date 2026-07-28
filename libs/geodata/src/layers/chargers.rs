//! EV charging locations from NDW's national OCPI dump (open data portal).
//! Output: nl-chargers.mbtiles, MVT layer `chargers`, point features, z8-z14.

use super::{read_gz, BuildCtx, BuildReport, Layer};
use crate::fetch::SourceSpec;
use crate::mvt::AttrVal;
use crate::tiler::{Tileset, TilesetConfig};
use crate::wkb::Geometry;

const ZMIN: u8 = 8;
const ZMAX: u8 = 14;

const NDW_OCPI: SourceSpec = SourceSpec {
    id: "ndw-chargers-ocpi",
    url: "https://opendata.ndw.nu/charging_point_locations_ocpi.json.gz",
    filename: "charging_point_locations_ocpi.json.gz",
    license: "Open (NDW open data portal)",
    attribution: "Nationaal Dataportaal Wegverkeer (NDW)",
    // The file refreshes about daily; being a national aggregate it does not
    // need to be fresher than a couple of days for a map overlay.
    recheck_days: 2,
    limit_rate: None,
};

pub struct ChargersLayer;

impl Layer for ChargersLayer {
    fn id(&self) -> &'static str {
        "chargers"
    }
    fn description(&self) -> &'static str {
        "EV charging locations, national OCPI aggregate (NDW open data)"
    }
    fn sources(&self) -> Vec<SourceSpec> {
        vec![NDW_OCPI]
    }

    fn build(&self, ctx: &BuildCtx) -> Result<BuildReport, String> {
        let path = ctx.cached(&NDW_OCPI);
        if !path.exists() {
            return Err("source not fetched yet (run: geodata fetch chargers)".into());
        }
        let bytes = read_gz(&path)?;
        let root: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|e| format!("parse OCPI json: {e}"))?;

        // OCPI dumps come either as a bare array of locations or wrapped in
        // {"data": [...]}. Find the location array defensively.
        let locations = if let Some(array) = root.as_array() {
            array.clone()
        } else if let Some(array) = root.get("data").and_then(|d| d.as_array()) {
            array.clone()
        } else {
            return Err("unrecognized OCPI structure (no location array)".into());
        };

        let mut tileset = Tileset::new();
        let mut features = 0u64;
        let mut skipped = 0u64;
        for location in &locations {
            let coords = location.get("coordinates");
            let lat = json_f64(coords.and_then(|c| c.get("latitude")));
            let lon = json_f64(coords.and_then(|c| c.get("longitude")));
            let (Some(lat), Some(lon)) = (lat, lon) else {
                skipped += 1;
                continue;
            };
            if !(50.0..54.0).contains(&lat) || !(3.0..8.0).contains(&lon) {
                skipped += 1;
                continue;
            }

            let mut attrs: Vec<(String, AttrVal)> = Vec::new();
            if let Some(name) = location.get("name").and_then(|v| v.as_str()) {
                if !name.is_empty() {
                    attrs.push(("name".into(), AttrVal::Str(name.into())));
                }
            }
            if let Some(op) = location
                .get("operator")
                .and_then(|o| o.get("name"))
                .and_then(|v| v.as_str())
            {
                attrs.push(("operator".into(), AttrVal::Str(op.into())));
            }
            if let Some(city) = location.get("city").and_then(|v| v.as_str()) {
                attrs.push(("city".into(), AttrVal::Str(city.into())));
            }
            if let Some(evses) = location.get("evses").and_then(|v| v.as_array()) {
                let mut connectors = 0u64;
                let mut max_kw = 0.0f64;
                for evse in evses {
                    if let Some(cs) = evse.get("connectors").and_then(|v| v.as_array()) {
                        connectors += cs.len() as u64;
                        for connector in cs {
                            let watts = json_f64(connector.get("max_electric_power"))
                                .or_else(|| {
                                    // OCPI 2.1: voltage * amperage
                                    let v = json_f64(connector.get("voltage"))?;
                                    let a = json_f64(connector.get("amperage"))?;
                                    Some(v * a)
                                })
                                .unwrap_or(0.0);
                            max_kw = max_kw.max(watts / 1000.0);
                        }
                    }
                }
                attrs.push(("evses".into(), AttrVal::Int(evses.len() as i64)));
                attrs.push(("connectors".into(), AttrVal::Int(connectors as i64)));
                if max_kw > 0.0 {
                    attrs.push(("max_kw".into(), AttrVal::Int(max_kw.round() as i64)));
                }
            }

            tileset.add("chargers", ZMIN, ZMAX, &Geometry::Point(lon, lat), &attrs);
            features += 1;
        }
        eprintln!("  chargers: {features} locations, {skipped} skipped");

        let out = ctx.out_file(self.id());
        let stats = tileset.finish(
            &out,
            &TilesetConfig {
                name: "nl-chargers".into(),
                description: "EV charging locations (NDW OCPI national aggregate)".into(),
                attribution: "NDW".into(),
                license: "Open data (NDW)".into(),
                minzoom: ZMIN,
                maxzoom: ZMAX,
            },
        )?;
        Ok(BuildReport {
            out_path: out,
            features,
            tiles: stats.tiles,
            bytes: stats.bytes,
        })
    }
}

fn json_f64(value: Option<&serde_json::Value>) -> Option<f64> {
    let value = value?;
    if let Some(f) = value.as_f64() {
        return Some(f);
    }
    value.as_str()?.trim().parse().ok()
}
