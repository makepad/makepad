//! Environmental noise from RIVM "Geluid in Nederland" (CC0): the nationwide
//! 10 m Lden all-sources raster, binned into 5 dB classes as gray8 class
//! tiles z6-z13. The class table ships in `geodata_classmap` metadata so the
//! renderer colormaps in the shader and the query side can name the class.
//! Source GeoTIFF is EPSG:28992 — sampled via the WGS84->RD polynomial.

use super::{BuildCtx, BuildReport, Layer};
use crate::fetch::SourceSpec;
use crate::raster::{build_raster, RasterConfig, RasterEncoding};
use crate::tiff::Tiff;
use std::path::PathBuf;
use std::process::Command;

const RIVM_NOISE: SourceSpec = SourceSpec {
    id: "rivm-lden",
    url: "https://data.rivm.nl/data/alo/rivm_20250801_Geluid_lden_allebronnen_2022.zip",
    filename: "rivm_geluid_lden_2022.zip",
    license: "CC0",
    attribution: "RIVM Geluid in Nederland (Lden 2022)",
    recheck_days: 365,
    limit_rate: Some("10M"),
};

const NL_BOUNDS: (f64, f64, f64, f64) = (3.2, 50.7, 7.3, 53.7);

/// Class thresholds in dB Lden: class i covers [edges[i-1], edges[i]).
const DB_EDGES: &[f64] = &[45.0, 50.0, 55.0, 60.0, 65.0, 70.0, 75.0];

pub struct NoiseLayer;

impl Layer for NoiseLayer {
    fn id(&self) -> &'static str {
        "noise"
    }
    fn description(&self) -> &'static str {
        "RIVM 10m Lden noise (all sources) as 5 dB class raster (CC0)"
    }
    fn sources(&self) -> Vec<SourceSpec> {
        vec![RIVM_NOISE]
    }

    fn build(&self, ctx: &BuildCtx) -> Result<BuildReport, String> {
        let zip = ctx.cached(&RIVM_NOISE);
        if !zip.exists() {
            return Err("source not fetched yet (run: geodata fetch noise)".into());
        }
        let tif_path = unzip_first_tif(&zip, &ctx.cache_dir)?;
        let mut tiff = Tiff::open(&tif_path)?;
        eprintln!(
            "  noise: {}x{} px, geo {:?}, nodata {:?}",
            tiff.width, tiff.height, tiff.geo, tiff.nodata
        );

        let mut sampler = move |lon: f64, lat: f64| -> Option<f32> {
            let (x, y) = crate::geo::wgs84_to_rd(lon, lat);
            let db = tiff.sample_geo(x, y)?;
            if db <= 0.0 {
                return None;
            }
            let mut class = 1u8;
            for (i, edge) in DB_EDGES.iter().enumerate() {
                if f64::from(db) >= *edge {
                    class = i as u8 + 2;
                }
            }
            Some(f32::from(class))
        };

        let classmap = serde_json::json!([
            {"class": 1, "label": "< 45 dB",   "color": "#00000000"},
            {"class": 2, "label": "45-50 dB", "color": "#4575b4"},
            {"class": 3, "label": "50-55 dB", "color": "#91bfdb"},
            {"class": 4, "label": "55-60 dB", "color": "#e0f382"},
            {"class": 5, "label": "60-65 dB", "color": "#fee090"},
            {"class": 6, "label": "65-70 dB", "color": "#fc8d59"},
            {"class": 7, "label": "70-75 dB", "color": "#d73027"},
            {"class": 8, "label": ">= 75 dB", "color": "#a50026"}
        ]);

        let out = ctx.out_file(self.id());
        let stats = build_raster(
            &out,
            &RasterConfig {
                name: "nl-noise".into(),
                description: "RIVM Lden all-sources noise, 5 dB classes".into(),
                attribution: "RIVM".into(),
                license: "CC0".into(),
                minzoom: 6,
                maxzoom: 13,
                bounds: NL_BOUNDS,
                encoding: RasterEncoding::ClassIndex,
                classmap: Some(classmap),
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

fn unzip_first_tif(zip: &PathBuf, cache_dir: &PathBuf) -> Result<PathBuf, String> {
    let listing = Command::new("unzip")
        .arg("-Z1")
        .arg(zip)
        .output()
        .map_err(|e| format!("unzip -Z1: {e}"))?;
    let names = String::from_utf8_lossy(&listing.stdout);
    let tif = names
        .lines()
        .find(|l| {
            let lower = l.to_lowercase();
            lower.ends_with(".tif") || lower.ends_with(".tiff")
        })
        .ok_or_else(|| format!("no .tif inside {} (members: {})", zip.display(), names))?
        .to_string();
    let out = cache_dir.join(std::path::Path::new(&tif).file_name().ok_or("bad name")?);
    if !out.exists() {
        let status = Command::new("unzip")
            .arg("-o")
            .arg("-j")
            .arg(zip)
            .arg(&tif)
            .arg("-d")
            .arg(cache_dir)
            .status()
            .map_err(|e| format!("unzip: {e}"))?;
        if !status.success() {
            return Err("unzip failed".into());
        }
    }
    Ok(out)
}
