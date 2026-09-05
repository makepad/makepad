//! Raster overlay tiles: sample any (lon, lat) -> value function into 256px
//! PNG tiles inside an .mbtiles (format=png). Two encodings:
//! - Terrarium RGB (elevation: e = h + 32768; R=e>>8, G=e&255, B=frac*256),
//!   the de-facto standard the renderer's future hillshade/3D terrain and
//!   map_nav's EV grade baking both read.
//! - Gray8 class index (noise/flood): pixel = class byte, 0 = no data; the
//!   class -> meaning/color table ships in metadata as `geodata_classmap`.

use crate::geo::tile_order_key;
use crate::png::{self, PngFormat};
use makepad_mbtile_reader::MbtilesWriter;
use std::collections::BTreeMap;
use std::path::Path;

pub const TILE_PX: u32 = 256;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RasterEncoding {
    Terrarium,
    ClassIndex,
}

impl RasterEncoding {
    fn as_str(&self) -> &'static str {
        match self {
            RasterEncoding::Terrarium => "terrarium",
            RasterEncoding::ClassIndex => "class-index",
        }
    }
}

pub struct RasterConfig {
    pub name: String,
    pub description: String,
    pub attribution: String,
    pub license: String,
    pub minzoom: u8,
    pub maxzoom: u8,
    /// lon/lat bbox to cover.
    pub bounds: (f64, f64, f64, f64),
    pub encoding: RasterEncoding,
    /// For ClassIndex: JSON array describing each class (index 1..).
    pub classmap: Option<serde_json::Value>,
}

pub struct RasterStats {
    pub tiles: u64,
    pub bytes: u64,
    pub skipped_empty: u64,
}

/// Build the raster pyramid. `sample(lon, lat)` returns the source value or
/// None for no-data; tiles that are entirely no-data are skipped.
pub fn build_raster(
    out_path: &Path,
    config: &RasterConfig,
    sample: &mut dyn FnMut(f64, f64) -> Option<f32>,
) -> Result<RasterStats, String> {
    let (min_lon, min_lat, max_lon, max_lat) = config.bounds;
    let mut tiles: BTreeMap<u128, (u8, u32, u32, Vec<u8>)> = BTreeMap::new();
    let mut stats = RasterStats {
        tiles: 0,
        bytes: 0,
        skipped_empty: 0,
    };

    for zoom in config.minzoom..=config.maxzoom {
        let scale = f64::from(1u32 << zoom);
        let (nx0, ny0) = crate::geo::wgs84_to_norm(min_lon, max_lat);
        let (nx1, ny1) = crate::geo::wgs84_to_norm(max_lon, min_lat);
        let tx0 = ((nx0 * scale).floor() as i64).clamp(0, (1 << zoom) - 1) as u32;
        let tx1 = ((nx1 * scale).floor() as i64).clamp(0, (1 << zoom) - 1) as u32;
        let ty0 = ((ny0 * scale).floor() as i64).clamp(0, (1 << zoom) - 1) as u32;
        let ty1 = ((ny1 * scale).floor() as i64).clamp(0, (1 << zoom) - 1) as u32;
        for ty in ty0..=ty1 {
            for tx in tx0..=tx1 {
                let (format, pixels, any) = render_tile(zoom, tx, ty, config.encoding, sample);
                if !any {
                    stats.skipped_empty += 1;
                    continue;
                }
                let data = png::encode(TILE_PX, TILE_PX, format, &pixels);
                stats.tiles += 1;
                stats.bytes += data.len() as u64;
                tiles.insert(tile_order_key(zoom, tx, ty), (zoom, tx, ty, data));
            }
        }
        eprintln!("  raster z{zoom}: {} tiles so far", stats.tiles);
    }

    let mut writer =
        MbtilesWriter::create(out_path).map_err(|e| format!("create mbtiles: {e:?}"))?;
    writer.set_metadata("name", &config.name);
    writer.set_metadata("description", &config.description);
    writer.set_metadata("format", "png");
    writer.set_metadata("type", "overlay");
    writer.set_metadata("minzoom", config.minzoom.to_string());
    writer.set_metadata("maxzoom", config.maxzoom.to_string());
    writer.set_metadata("attribution", &config.attribution);
    writer.set_metadata("license", &config.license);
    writer.set_metadata(
        "bounds",
        format!("{min_lon:.6},{min_lat:.6},{max_lon:.6},{max_lat:.6}"),
    );
    writer.set_metadata("geodata_encoding", config.encoding.as_str());
    if let Some(classmap) = &config.classmap {
        writer.set_metadata("geodata_classmap", classmap.to_string());
    }
    writer.set_metadata(
        "geodata_built_unix",
        crate::clock::now_unix().to_string(),
    );
    for (_, (zoom, x, y, data)) in tiles {
        writer
            .write_tile_xyz(zoom, x, y, &data)
            .map_err(|e| format!("write tile z{zoom}/{x}/{y}: {e:?}"))?;
    }
    writer.finish().map_err(|e| format!("finish: {e:?}"))?;
    Ok(stats)
}

fn render_tile(
    zoom: u8,
    tx: u32,
    ty: u32,
    encoding: RasterEncoding,
    sample: &mut dyn FnMut(f64, f64) -> Option<f32>,
) -> (PngFormat, Vec<u8>, bool) {
    let scale = f64::from(1u32 << zoom);
    let mut any = false;
    match encoding {
        RasterEncoding::Terrarium => {
            let mut pixels = vec![0u8; (TILE_PX * TILE_PX * 3) as usize];
            for py in 0..TILE_PX {
                for px in 0..TILE_PX {
                    let nx = (f64::from(tx) + (f64::from(px) + 0.5) / f64::from(TILE_PX)) / scale;
                    let ny = (f64::from(ty) + (f64::from(py) + 0.5) / f64::from(TILE_PX)) / scale;
                    let p = makepad_map_nav::geo::norm_to_lon_lat(nx, ny);
                    let height = match sample(p.lon, p.lat) {
                        Some(h) => {
                            any = true;
                            f64::from(h)
                        }
                        None => 0.0,
                    };
                    let e = (height + 32_768.0).clamp(0.0, 65_535.996);
                    let i = ((py * TILE_PX + px) * 3) as usize;
                    pixels[i] = (e / 256.0) as u8;
                    pixels[i + 1] = (e as u32 % 256) as u8;
                    pixels[i + 2] = (e.fract() * 256.0) as u8;
                }
            }
            (PngFormat::Rgb8, pixels, any)
        }
        RasterEncoding::ClassIndex => {
            let mut pixels = vec![0u8; (TILE_PX * TILE_PX) as usize];
            for py in 0..TILE_PX {
                for px in 0..TILE_PX {
                    let nx = (f64::from(tx) + (f64::from(px) + 0.5) / f64::from(TILE_PX)) / scale;
                    let ny = (f64::from(ty) + (f64::from(py) + 0.5) / f64::from(TILE_PX)) / scale;
                    let p = makepad_map_nav::geo::norm_to_lon_lat(nx, ny);
                    if let Some(class) = sample(p.lon, p.lat) {
                        let class = (class as i64).clamp(0, 255) as u8;
                        if class > 0 {
                            any = true;
                        }
                        pixels[(py * TILE_PX + px) as usize] = class;
                    }
                }
            }
            (PngFormat::Gray8, pixels, any)
        }
    }
}
