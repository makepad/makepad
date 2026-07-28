//! Elevation from Copernicus GLO-30 (30 m DSM, ESA/Airbus, attribution
//! license) as terrarium-encoded raster tiles, z6-z12 over NL.
//! Consumers: renderer hillshade / 3D terrain, and map_nav's per-edge
//! climb/descent baking for EV routing (both read the same file).
//!
//! Source: anonymous AWS open-data COGs, one 1x1 degree tile each. Tiles
//! that are all-ocean don't exist upstream — those fetches are expected to
//! fail and the build treats missing cells as sea level.

use super::{BuildCtx, BuildReport, Layer};
use crate::fetch::{fetch_source, FetchOptions, SourceSpec};
use crate::raster::{build_raster, RasterConfig, RasterEncoding};
use crate::tiff::Tiff;
use std::collections::HashMap;

const LAT_RANGE: std::ops::RangeInclusive<i32> = 50..=53;
const LON_RANGE: std::ops::RangeInclusive<i32> = 3..=7;
const NL_BOUNDS: (f64, f64, f64, f64) = (3.0, 50.7, 7.3, 53.8);

fn tile_specs() -> Vec<SourceSpec> {
    let mut specs = Vec::new();
    for lat in LAT_RANGE {
        for lon in LON_RANGE {
            let stem = format!("Copernicus_DSM_COG_10_N{lat:02}_00_E{lon:03}_00_DEM");
            let url: &'static str = Box::leak(
                format!("https://copernicus-dem-30m.s3.amazonaws.com/{stem}/{stem}.tif")
                    .into_boxed_str(),
            );
            let filename: &'static str =
                Box::leak(format!("glo30_N{lat:02}_E{lon:03}.tif").into_boxed_str());
            let id: &'static str =
                Box::leak(format!("glo30-N{lat:02}-E{lon:03}").into_boxed_str());
            specs.push(SourceSpec {
                id,
                url,
                filename,
                license: "Copernicus DEM (ESA/Airbus, attribution)",
                attribution: "Copernicus DEM GLO-30 (c) ESA / Airbus",
                recheck_days: 3650, // static dataset
                limit_rate: None,
            });
        }
    }
    specs
}

pub struct TerrainLayer;

impl Layer for TerrainLayer {
    fn id(&self) -> &'static str {
        "terrain"
    }
    fn description(&self) -> &'static str {
        "Copernicus GLO-30 elevation as terrarium tiles (hillshade + EV grades)"
    }
    fn sources(&self) -> Vec<SourceSpec> {
        tile_specs()
    }

    fn build(&self, ctx: &BuildCtx) -> Result<BuildReport, String> {
        // Fetch tolerantly here: all-ocean 1-degree cells 404 upstream.
        let opts = FetchOptions {
            cache_dir: ctx.cache_dir.clone(),
            force: false,
        };
        let mut cells: HashMap<(i32, i32), Option<Tiff>> = HashMap::new();
        for (index, spec) in tile_specs().iter().enumerate() {
            let lat = 50 + (index as i32) / 5;
            let lon = 3 + (index as i32) % 5;
            let path = ctx.cached(spec);
            if !path.exists() {
                match fetch_source(&opts, spec) {
                    Ok(_) => {}
                    Err(error) => {
                        eprintln!("  terrain: {} unavailable ({error}) — treating as sea", spec.id);
                        cells.insert((lat, lon), None);
                        continue;
                    }
                }
            }
            match Tiff::open(&path) {
                Ok(tiff) => {
                    cells.insert((lat, lon), Some(tiff));
                }
                Err(error) => {
                    eprintln!("  terrain: {} unreadable ({error})", spec.id);
                    cells.insert((lat, lon), None);
                }
            }
        }
        let available = cells.values().filter(|c| c.is_some()).count();
        eprintln!("  terrain: {available} of {} degree cells available", cells.len());
        if available == 0 {
            return Err("no GLO-30 cells available".into());
        }

        let mut sampler = move |lon: f64, lat: f64| -> Option<f32> {
            let cell = (lat.floor() as i32, lon.floor() as i32);
            match cells.get_mut(&cell) {
                Some(Some(tiff)) => tiff.sample_geo(lon, lat).or(Some(0.0)),
                Some(None) => Some(0.0), // known-ocean cell: sea level
                None => None,            // outside our cell set
            }
        };

        let out = ctx.out_file(self.id());
        let stats = build_raster(
            &out,
            &RasterConfig {
                name: "nl-terrain".into(),
                description: "Copernicus GLO-30 elevation, terrarium encoding".into(),
                attribution: "Copernicus DEM GLO-30 (c) ESA / Airbus".into(),
                license: "Copernicus DEM licence (attribution)".into(),
                minzoom: 6,
                maxzoom: 12,
                bounds: NL_BOUNDS,
                encoding: RasterEncoding::Terrarium,
                classmap: None,
            },
            &mut sampler,
        )?;
        Ok(BuildReport {
            out_path: out,
            features: stats.tiles,
            tiles: stats.tiles,
            bytes: stats.bytes,
        })
    }
}
