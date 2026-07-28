//! CBS Wijk- en Buurtkaart: municipality/district/neighborhood polygons with
//! kerncijfers (CC BY 4.0). Output: nl-wijkbuurt.mbtiles with MVT layers
//! `gemeenten` (z6-z8), `wijken` (z9-z10), `buurten` (z11-z13).

use super::{unzip_gpkgs, BuildCtx, BuildReport, Layer};
use crate::fetch::SourceSpec;
use crate::gpkg::Gpkg;
use crate::mvt::AttrVal;
use crate::tiler::{Tileset, TilesetConfig};
use makepad_mbtile_reader::Value;

const WIJKBUURT: SourceSpec = SourceSpec {
    id: "cbs-wijkbuurt",
    url: "https://geodata.cbs.nl/files/Wijkenbuurtkaart/WijkBuurtkaart_2025_v1.zip",
    filename: "wijkbuurtkaart_2025.zip",
    license: "CC BY 4.0",
    attribution: "CBS / Kadaster",
    recheck_days: 90,
    limit_rate: None,
};

pub struct WijkBuurtLayer;

impl Layer for WijkBuurtLayer {
    fn id(&self) -> &'static str {
        "wijkbuurt"
    }
    fn description(&self) -> &'static str {
        "CBS municipality/district/neighborhood polygons with key statistics (CC BY 4.0)"
    }
    fn sources(&self) -> Vec<SourceSpec> {
        vec![WIJKBUURT]
    }

    fn build(&self, ctx: &BuildCtx) -> Result<BuildReport, String> {
        let zip_path = ctx.cached(&WIJKBUURT);
        if !zip_path.exists() {
            return Err("source not fetched yet (run: geodata fetch wijkbuurt)".into());
        }
        let gpkgs = unzip_gpkgs(&zip_path, &ctx.cache_dir)?;
        let mut tileset = Tileset::new();
        tileset.query_rings(&["gemeenten", "wijken", "buurten"]);
        let mut features = 0u64;
        for gpkg_path in &gpkgs {
            let mut gpkg = Gpkg::open(gpkg_path)?;
            for table in gpkg.feature_tables()? {
                let lower = table.table.to_lowercase();
                let (mvt_layer, zmin, zmax) = if lower.contains("gemeente") {
                    ("gemeenten", 6u8, 8u8)
                } else if lower.contains("wijk") {
                    ("wijken", 9, 10)
                } else if lower.contains("buurt") {
                    ("buurten", 11, 13)
                } else {
                    continue;
                };
                eprintln!(
                    "  wijkbuurt: table {} -> layer {} ({} columns)",
                    table.table,
                    mvt_layer,
                    table.columns.len()
                );
                gpkg.for_each_feature(&table, |_rowid, values, geom| {
                    let mut attrs: Vec<(String, AttrVal)> = Vec::new();
                    for (index, column) in table.columns.iter().enumerate() {
                        if index == table.geom_col {
                            continue;
                        }
                        match values.get(index) {
                            // Negative values are CBS suppression sentinels.
                            Some(Value::Integer(i)) if *i >= 0 => {
                                attrs.push((column.to_lowercase(), AttrVal::Int(*i)));
                            }
                            Some(Value::Float(f)) if *f >= 0.0 => {
                                attrs.push((column.to_lowercase(), AttrVal::Float(*f)));
                            }
                            Some(Value::Text(t)) if !t.is_empty() && t.len() < 80 => {
                                attrs.push((column.to_lowercase(), AttrVal::Str(t.clone())));
                            }
                            _ => {}
                        }
                    }
                    tileset.add(mvt_layer, zmin, zmax, &geom, &attrs);
                    features += 1;
                })?;
            }
        }

        let out = ctx.out_file(self.id());
        let stats = tileset.finish(
            &out,
            &TilesetConfig {
                name: "nl-wijkbuurt".into(),
                description: "CBS Wijk- en Buurtkaart with kerncijfers".into(),
                attribution: "CBS / Kadaster".into(),
                license: "CC BY 4.0".into(),
                minzoom: 6,
                maxzoom: 13,
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
