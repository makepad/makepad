//! BAG building polygons with construction year, z13-z14 (CC0, PDOK).
//! Output: nl-buildings-age.mbtiles, MVT layer `bag`.
//!
//! This is the one layer that cannot use the in-memory `Tileset`: ~10.9M
//! polygons at two zoom levels. It streams instead: one pass over the GPKG
//! clips features and spools compact per-256x256-block files to disk, then
//! blocks are loaded one at a time (in writer rowid order) and encoded.
//! NL spans only a handful of blocks at z13/z14, so peak memory is one
//! block's features, not the country's.

use super::{BuildCtx, BuildReport, Layer};
use crate::fetch::SourceSpec;
use crate::spool::SpoolTiler;
use crate::gpkg::Gpkg;
use crate::mvt::AttrVal;
use makepad_mbtile_reader::Value;

const ZMIN: u8 = 13;
const ZMAX: u8 = 14;

const BAG_LIGHT: SourceSpec = SourceSpec {
    id: "bag-light",
    url: "https://service.pdok.nl/lv/bag/atom/downloads/bag-light.gpkg",
    filename: "bag-light.gpkg",
    license: "CC0",
    attribution: "Kadaster BAG via PDOK",
    // Monthly refresh upstream; a stale building year is harmless.
    recheck_days: 45,
    limit_rate: None,
};

pub struct BuildingsLayer;

impl Layer for BuildingsLayer {
    fn id(&self) -> &'static str {
        "buildings-age"
    }
    fn description(&self) -> &'static str {
        "BAG building polygons with construction year (PDOK bag-light, CC0, 7.8 GB)"
    }
    fn sources(&self) -> Vec<SourceSpec> {
        vec![BAG_LIGHT]
    }

    fn build(&self, ctx: &BuildCtx) -> Result<BuildReport, String> {
        let path = ctx.cached(&BAG_LIGHT);
        if !path.exists() {
            return Err("source not fetched yet (run: geodata fetch buildings-age)".into());
        }
        let mut gpkg = Gpkg::open(&path)?;
        let tables = gpkg.feature_tables()?;
        let table = tables
            .iter()
            .find(|t| t.table.eq_ignore_ascii_case("pand"))
            .ok_or_else(|| {
                format!(
                    "no 'pand' feature table; tables: {:?}",
                    tables.iter().map(|t| &t.table).collect::<Vec<_>>()
                )
            })?;
        eprintln!(
            "  bag: table {} columns {:?} (srs {})",
            table.table, table.columns, table.srs_id
        );
        let bouwjaar_col = table
            .columns
            .iter()
            .position(|c| c.eq_ignore_ascii_case("bouwjaar"))
            .ok_or("no bouwjaar column")?;
        let status_col = table
            .columns
            .iter()
            .position(|c| c.eq_ignore_ascii_case("status"));

        let spool_dir = ctx.cache_dir.join("spool-buildings");
        let mut tiler = SpoolTiler::new(&spool_dir, ZMIN, ZMAX)?;
        let mut features = 0u64;
        let skipped = gpkg.for_each_feature(table, |_rowid, values, geom| {
            let mut attrs: Vec<(String, AttrVal)> = Vec::new();
            match values.get(bouwjaar_col) {
                Some(Value::Integer(year)) if *year > 0 => {
                    attrs.push(("bouwjaar".into(), AttrVal::Int(*year)));
                }
                _ => {}
            }
            if let Some(col) = status_col {
                if let Some(Value::Text(status)) = values.get(col) {
                    if !status.is_empty() {
                        attrs.push(("status".into(), AttrVal::Str(status.clone())));
                    }
                }
            }
            if tiler.add("bag", &geom, &attrs).is_ok() {
                features += 1;
                if features % 1_000_000 == 0 {
                    eprintln!("  bag: {} M buildings spooled", features / 1_000_000);
                }
            }
        })?;
        eprintln!("  bag: {features} buildings, {skipped} without usable geometry");

        let out = ctx.out_file(self.id());
        let stats = tiler.finish(
            &out,
            &crate::tiler::TilesetConfig {
                name: "nl-buildings-age".into(),
                description: "BAG buildings with construction year".into(),
                attribution: "Kadaster BAG via PDOK".into(),
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
