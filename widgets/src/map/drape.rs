//! Drape landcover colors over a terrain hillshade render.
//!
//! Shared by the map apps (examples/map, apps/route): reads Shortbread
//! landcover polygons from the base mbtiles at the shade's zoom, rasterizes
//! them into a class mask and blends palette colors lit by the hillshade
//! into the RGBA texture, fading out across the treeline.

use crate::map::geometry::TileKey;
use crate::map::tile::{decode_vector_tile_payload, parse_mvt_tile, MvtSink};
use makepad_mbtile_reader::MbtilesReader;
use std::collections::HashMap;

pub fn drape_landcover(
    reader: &mut MbtilesReader,
    bbox: (f64, f64, f64, f64),
    w: usize,
    h: usize,
    elev: &[f32],
    shade: &[u8],
    rgba: &mut [u8],
) {
    let (west, north, east, south) = bbox;
    let span_x = (east - west).max(1e-12);
    let span_y = (south - north).max(1e-12);
    // Landcover at the shade's own zoom for crisp forest edges, backed
    // off only if the tile fetch would explode.
    let want = (w as f64 / (span_x * 256.0)).log2().ceil() as i64;
    let mut z = want.clamp(6, 12) as u32;
    loop {
        let nt = 1_i64 << z;
        let count = (((east * nt as f64).floor() - (west * nt as f64).floor()) + 1.0)
            * (((south * nt as f64).floor() - (north * nt as f64).floor()) + 1.0);
        if count <= 400.0 || z <= 6 {
            break;
        }
        z -= 1;
    }
    let n_tiles = 1_i64 << z;

    struct LandSink {
        // Even-odd polygon rings in shade-pixel coords with a palette id.
        polys: Vec<(Vec<(f32, f32)>, u8)>,
        origin: (f64, f64),
        scale: (f64, f64),
        next_id: u64,
    }
    // Palette: 1 wood, 2 grass/farm, 3 scrub/heath, 4 water, 5 glacier.
    const PALETTE: [[u8; 3]; 6] = [
        [0, 0, 0],
        [88, 126, 82],
        [140, 165, 110],
        [120, 144, 96],
        [110, 148, 176],
        [238, 242, 246],
    ];
    impl MvtSink for LandSink {
        fn alloc_feature_id(&mut self) -> u64 {
            self.next_id += 1;
            self.next_id
        }
        fn add_point(
            &mut self,
            _tile_key: TileKey,
            _extent: u32,
            _point: (i32, i32),
            _tags: HashMap<String, String>,
        ) {
        }
        fn add_path(
            &mut self,
            tile_key: TileKey,
            extent: u32,
            points: &[(i32, i32)],
            tags: HashMap<String, String>,
            close: bool,
        ) {
            if !close || points.len() < 3 {
                return;
            }
            let layer = tags.get("layer").map(|v| v.as_str()).unwrap_or("");
            let kind = tags.get("kind").map(|v| v.as_str()).unwrap_or("");
            let class: u8 = match layer {
                "water_polygons" | "ocean" => 4,
                "land" | "landuse" | "landcover" | "nature" | "sites" => match kind {
                    "forest" | "wood" => 1,
                    "grass" | "grassland" | "meadow" | "farmland" | "orchard"
                    | "allotments" | "vineyard" | "garden" | "park" | "village_green" => 2,
                    "scrub" | "heath" | "bare_rock" if kind == "scrub" || kind == "heath" => 3,
                    "glacier" => 5,
                    _ => 0,
                },
                _ => 0,
            };
            if class == 0 {
                return;
            }
            let tz = 1u32 << tile_key.z;
            let ring: Vec<(f32, f32)> = points
                .iter()
                .map(|&(px, py)| {
                    let nx = (tile_key.x as f64 + px as f64 / extent as f64) / tz as f64;
                    let ny = (tile_key.y as f64 + py as f64 / extent as f64) / tz as f64;
                    (
                        ((nx - self.origin.0) * self.scale.0) as f32,
                        ((ny - self.origin.1) * self.scale.1) as f32,
                    )
                })
                .collect();
            self.polys.push((ring, class));
        }
    }

    let mut sink = LandSink {
        polys: Vec::new(),
        origin: (west, north),
        scale: (w as f64 / span_x, h as f64 / span_y),
        next_id: 0,
    };
    let tx0 = ((west * n_tiles as f64).floor() as i64).clamp(0, n_tiles - 1);
    let tx1 = ((east * n_tiles as f64).floor() as i64).clamp(0, n_tiles - 1);
    let ty0 = ((north * n_tiles as f64).floor() as i64).clamp(0, n_tiles - 1);
    let ty1 = ((south * n_tiles as f64).floor() as i64).clamp(0, n_tiles - 1);
    for ty in ty0..=ty1 {
        for tx in tx0..=tx1 {
            let tms_row = n_tiles - 1 - ty;
            let Ok(Some(raw)) = reader.get_tile(z as i64, tx, tms_row) else {
                continue;
            };
            let Ok(data) = decode_vector_tile_payload(&raw) else {
                continue;
            };
            let key = TileKey {
                z,
                x: tx as i32,
                y: ty as i32,
            };
            let _ = parse_mvt_tile(&data, key, &mut sink);
        }
    }

    // Scanline even-odd fill into a full-resolution class mask.
    let (mw, mh) = (w, h);
    let mut mask = vec![0u8; mw * mh];
    let mut xs: Vec<f32> = Vec::new();
    for (ring, class) in &sink.polys {
        let min_y = ring.iter().map(|p| p.1).fold(f32::MAX, f32::min);
        let max_y = ring.iter().map(|p| p.1).fold(f32::MIN, f32::max);
        let y0 = (min_y.floor().max(0.0)) as usize;
        let y1 = (max_y.ceil().min(mh as f32 - 1.0)) as usize;
        for my in y0..=y1.min(mh - 1) {
            let sy = my as f32 + 0.5;
            xs.clear();
            for i in 0..ring.len() {
                let (x1p, y1p) = ring[i];
                let (x2p, y2p) = ring[(i + 1) % ring.len()];
                if (y1p <= sy && y2p > sy) || (y2p <= sy && y1p > sy) {
                    xs.push(x1p + (sy - y1p) / (y2p - y1p) * (x2p - x1p));
                }
            }
            xs.sort_unstable_by(|a, b| a.total_cmp(b));
            for pair in xs.chunks_exact(2) {
                let a = (pair[0].max(0.0)) as usize;
                let b = (pair[1].min(mw as f32 - 1.0)) as usize;
                for mx in a..=b.min(mw - 1) {
                    mask[my * mw + mx] = *class;
                }
            }
        }
    }

    // Blend: landcover color, lit by the hillshade, below the treeline.
    for y in 0..h {
        for x in 0..w {
            let class = mask[y * mw + x] as usize;
            if class == 0 {
                continue;
            }
            let i = y * w + x;
            if rgba[i * 4 + 3] == 0 {
                continue; // outside coverage stays transparent
            }
            let e = elev[i];
            // Fade landcover out across the treeline into rock/snow.
            let t = if class == 4 || class == 5 {
                1.0
            } else {
                (1.0 - (e - 1750.0) / 300.0).clamp(0.0, 1.0)
            };
            if t <= 0.0 {
                continue;
            }
            let light = shade[i] as f32 / 255.0;
            let c = PALETTE[class];
            for ch in 0..3 {
                let lc = c[ch] as f32 * light;
                let base = rgba[i * 4 + ch] as f32;
                rgba[i * 4 + ch] = (base * (1.0 - t) + lc * t) as u8;
            }
        }
    }
}
