//! Protected-nature polygons: Natura 2000 + Ramsar wetlands (PDOK, CC0).
//! Output: nl-nature.mbtiles, MVT layers `natura2000` and `wetlands`, z6-z12.

use super::{BuildCtx, BuildReport, Layer};
use crate::fetch::SourceSpec;
use crate::gpkg::Gpkg;
use crate::mvt::AttrVal;
use crate::tiler::{Tileset, TilesetConfig};
use makepad_mbtile_reader::Value;

const ZMIN: u8 = 6;
const ZMAX: u8 = 12;

const NATURA2000: SourceSpec = SourceSpec {
    id: "natura2000",
    url: "https://service.pdok.nl/rvo/natura2000/atom/downloads/natura2000.gpkg",
    filename: "natura2000.gpkg",
    license: "CC0",
    attribution: "Rijksdienst voor Ondernemend Nederland via PDOK",
    recheck_days: 30,
    limit_rate: Some("10M"),
};

const WETLANDS: SourceSpec = SourceSpec {
    id: "wetlands",
    url: "https://service.pdok.nl/rvo/wetlands/atom/downloads/wetlands.gpkg",
    filename: "wetlands.gpkg",
    license: "CC0",
    attribution: "Rijksdienst voor Ondernemend Nederland via PDOK",
    recheck_days: 30,
    limit_rate: Some("10M"),
};

pub struct NatureLayer;

impl Layer for NatureLayer {
    fn id(&self) -> &'static str {
        "nature"
    }
    fn description(&self) -> &'static str {
        "Protected nature polygons: Natura 2000 + Ramsar wetlands (PDOK, CC0)"
    }
    fn sources(&self) -> Vec<SourceSpec> {
        vec![NATURA2000, WETLANDS]
    }

    fn build(&self, ctx: &BuildCtx) -> Result<BuildReport, String> {
        let mut tileset = Tileset::new();
        tileset.query_rings(&["natura2000", "wetlands"]);
        let mut features = 0u64;
        for (spec, mvt_layer) in [(&NATURA2000, "natura2000"), (&WETLANDS, "wetlands")] {
            let path = ctx.cached(spec);
            if !path.exists() {
                return Err(format!(
                    "source {} not fetched yet (run: geodata fetch nature)",
                    spec.id
                ));
            }
            let mut gpkg = Gpkg::open(&path)?;
            for table in gpkg.feature_tables()? {
                eprintln!(
                    "  {}: table {} columns {:?} (srs {})",
                    spec.id, table.table, table.columns, table.srs_id
                );
                gpkg.for_each_feature(&table, |_rowid, values, geom| {
                    let mut attrs: Vec<(String, AttrVal)> = Vec::new();
                    for (index, column) in table.columns.iter().enumerate() {
                        if index == table.geom_col {
                            continue;
                        }
                        match values.get(index) {
                            Some(Value::Text(text)) if !text.is_empty() && text.len() < 120 => {
                                attrs.push((
                                    column.to_lowercase(),
                                    AttrVal::Str(text.clone()),
                                ));
                            }
                            Some(Value::Integer(i)) => {
                                attrs.push((column.to_lowercase(), AttrVal::Int(*i)));
                            }
                            _ => {}
                        }
                    }
                    attrs.push(("kind".into(), AttrVal::Str(mvt_layer.into())));
                    tileset.add(mvt_layer, ZMIN, ZMAX, &geom, &attrs);
                    features += 1;
                })?;
            }
        }

        let out = ctx.out_file(self.id());
        let stats = tileset.finish(
            &out,
            &TilesetConfig {
                name: "nl-nature".into(),
                description: "Protected nature: Natura 2000 + Ramsar wetlands (NL)".into(),
                attribution: "RVO via PDOK".into(),
                license: "CC0".into(),
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
