//! CBS Vierkantstatistieken: 500m and 100m statistics grids (CC-BY 4.0).
//! Output: nl-demographics.mbtiles, MVT layers `vk500` (z8-z11) and
//! `vk100` (z12-z13), square polygons carrying the per-cell statistics.
//! Negative values are CBS suppression sentinels and are omitted.

use super::{unzip_gpkgs, BuildCtx, BuildReport, Layer};
use crate::fetch::SourceSpec;
use crate::gpkg::Gpkg;
use crate::mvt::AttrVal;
use crate::tiler::{Tileset, TilesetConfig};
use makepad_mbtile_reader::Value;

const VK500: SourceSpec = SourceSpec {
    id: "cbs-vk500",
    url: "https://download.cbs.nl/vierkant/500/2026-cbs_vk500_2025_v1.zip",
    filename: "cbs_vk500_2025.zip",
    license: "CC BY 4.0",
    attribution: "Centraal Bureau voor de Statistiek (CBS)",
    recheck_days: 90,
    limit_rate: None,
};

const VK100: SourceSpec = SourceSpec {
    id: "cbs-vk100",
    url: "https://download.cbs.nl/vierkant/100/2026-cbs_vk100_2025_v1.zip",
    filename: "cbs_vk100_2025.zip",
    license: "CC BY 4.0",
    attribution: "Centraal Bureau voor de Statistiek (CBS)",
    recheck_days: 90,
    limit_rate: None,
};

pub struct CbsGridLayer;

impl Layer for CbsGridLayer {
    fn id(&self) -> &'static str {
        "demographics"
    }
    fn description(&self) -> &'static str {
        "CBS 500m/100m statistics grids: population, age, housing (CC BY 4.0)"
    }
    fn sources(&self) -> Vec<SourceSpec> {
        vec![VK500, VK100]
    }

    fn build(&self, ctx: &BuildCtx) -> Result<BuildReport, String> {
        let mut tileset = Tileset::new();
        let mut features = 0u64;
        for (spec, mvt_layer, zmin, zmax) in
            [(&VK500, "vk500", 8u8, 11u8), (&VK100, "vk100", 12u8, 13u8)]
        {
            let zip_path = ctx.cached(spec);
            if !zip_path.exists() {
                return Err(format!(
                    "source {} not fetched yet (run: geodata fetch demographics)",
                    spec.id
                ));
            }
            let gpkg_path = unzip_gpkgs(&zip_path, &ctx.cache_dir)?
                .into_iter()
                .next()
                .ok_or("no gpkg extracted")?;
            let mut gpkg = Gpkg::open(&gpkg_path)?;
            for table in gpkg.feature_tables()? {
                eprintln!(
                    "  {}: table {} ({} columns, srs {})",
                    spec.id,
                    table.table,
                    table.columns.len(),
                    table.srs_id
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
                            _ => {}
                        }
                    }
                    if attrs.is_empty() {
                        return;
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
                name: "nl-demographics".into(),
                description: "CBS 500m/100m statistics grids".into(),
                attribution: "CBS".into(),
                license: "CC BY 4.0".into(),
                minzoom: 8,
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

