//! Flood hazard from the JRC Europe river flood map (CC BY 4.0): maximum
//! water depth for the 1-in-100-year event, binned into depth classes as
//! gray8 class tiles z6-z11 over NL. WGS84 source (~90 m), sampled directly.
//!
//! Follow-ups documented in the README: the JRC `spurious_depth_areas` mask
//! (removes a few known artifacts) and the PDOK ROR official zone polygons
//! (CC0 GML) as a vector companion layer.

use super::{BuildCtx, BuildReport, Layer};
use crate::fetch::SourceSpec;
use crate::raster::{build_raster, RasterConfig, RasterEncoding};
use crate::tiff::Tiff;

const JRC_RP100: SourceSpec = SourceSpec {
    id: "jrc-flood-rp100",
    url: "https://jeodpp.jrc.ec.europa.eu/ftp/jrc-opendata/CEMS-EFAS/flood_hazard/Europe_RP100_filled_depth.tif",
    filename: "jrc_europe_rp100_depth.tif",
    license: "CC BY 4.0",
    attribution: "JRC European Commission, river flood hazard maps",
    recheck_days: 365,
    limit_rate: None,
};

const NL_BOUNDS: (f64, f64, f64, f64) = (3.2, 50.7, 7.3, 53.7);

/// Depth class edges in meters: class i covers [edges[i-1], edges[i]).
const DEPTH_EDGES: &[f64] = &[0.5, 1.0, 2.0, 4.0];

pub struct FloodLayer;

impl Layer for FloodLayer {
    fn id(&self) -> &'static str {
        "flood"
    }
    fn description(&self) -> &'static str {
        "JRC 1-in-100y river flood depth as class raster (CC BY 4.0)"
    }
    fn sources(&self) -> Vec<SourceSpec> {
        vec![JRC_RP100]
    }

    fn build(&self, ctx: &BuildCtx) -> Result<BuildReport, String> {
        let path = ctx.cached(&JRC_RP100);
        if !path.exists() {
            return Err("source not fetched yet (run: geodata fetch flood)".into());
        }
        let mut tiff = Tiff::open(&path)?;
        eprintln!(
            "  flood: {}x{} px, geo {:?}, nodata {:?}",
            tiff.width, tiff.height, tiff.geo, tiff.nodata
        );

        let mut sampler = move |lon: f64, lat: f64| -> Option<f32> {
            let depth = tiff.sample_geo(lon, lat)?;
            if depth <= 0.0 {
                return None;
            }
            let mut class = 1u8;
            for (i, edge) in DEPTH_EDGES.iter().enumerate() {
                if f64::from(depth) >= *edge {
                    class = i as u8 + 2;
                }
            }
            Some(f32::from(class))
        };

        let classmap = serde_json::json!([
            {"class": 1, "label": "< 0.5 m", "color": "#c6dbef"},
            {"class": 2, "label": "0.5-1 m", "color": "#9ecae1"},
            {"class": 3, "label": "1-2 m",   "color": "#6baed6"},
            {"class": 4, "label": "2-4 m",   "color": "#3182bd"},
            {"class": 5, "label": ">= 4 m",  "color": "#08519c"}
        ]);

        let out = ctx.out_file(self.id());
        let stats = build_raster(
            &out,
            &RasterConfig {
                name: "nl-flood".into(),
                description: "JRC river flood depth RP100, classed".into(),
                attribution: "JRC European Commission".into(),
                license: "CC BY 4.0".into(),
                minzoom: 6,
                maxzoom: 11,
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
