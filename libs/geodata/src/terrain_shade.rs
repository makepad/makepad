//! Hillshade + hypsometric tint from the terrarium-encoded terrain
//! overlay (nl-terrain.mbtiles, GLO-30). Produces a mercator-aligned RGBA
//! texture for an arbitrary view bbox: the renderer draws it as a quad
//! between the land fills and the road network.
//!
//! Sun matches the building walls (northwest, ~45° altitude) so the whole
//! 3D scene reads as one light source.

use crate::png;
use makepad_mbtile_reader::MbtilesReader;
use std::collections::HashMap;
use std::path::Path;

const TILE_PX: usize = 256;

pub struct TerrainShader {
    reader: MbtilesReader,
    min_zoom: u32,
    max_zoom: u32,
    /// Decoded elevation tiles (z, x, y) → 256x256 meters grid.
    cache: HashMap<(u32, i64, i64), Option<Vec<f32>>>,
    /// Sun direction (map space: x east, y south/screen-down, z up),
    /// normalized at use. Defaults to the legacy hillshade sun; set it from
    /// the app's `SceneSun` so the whole scene reads as one light source.
    pub sun: (f32, f32, f32),
}

fn terrarium_decode(png: &png::DecodedPng) -> Option<Vec<f32>> {
    let bpp = png.format.bytes_per_pixel();
    if bpp < 3 || png.width as usize != TILE_PX || png.height as usize != TILE_PX {
        return None;
    }
    let mut out = Vec::with_capacity(TILE_PX * TILE_PX);
    for px in png.pixels.chunks_exact(bpp) {
        out.push(px[0] as f32 * 256.0 + px[1] as f32 + px[2] as f32 / 256.0 - 32768.0);
    }
    Some(out)
}

impl TerrainShader {
    pub fn open(path: &Path) -> Result<Self, String> {
        let mut reader = MbtilesReader::open(path).map_err(|e| format!("open terrain: {e}"))?;
        let (min_zoom, max_zoom) = reader
            .get_metadata()
            .ok()
            .and_then(|meta| {
                let get = |k: &str| {
                    meta.iter()
                        .find(|(name, _)| name.as_str() == k)
                        .and_then(|(_, v)| v.parse::<u32>().ok())
                };
                Some((get("minzoom")?, get("maxzoom")?))
            })
            .unwrap_or((6, 12));
        Ok(Self {
            reader,
            min_zoom,
            max_zoom,
            cache: HashMap::new(),
            sun: (-0.5, -0.62, 0.6),
        })
    }

    fn tile(&mut self, z: u32, x: i64, y: i64) -> Option<&Vec<f32>> {
        let key = (z, x, y);
        if !self.cache.contains_key(&key) {
            let n = 1i64 << z;
            let decoded = if x < 0 || y < 0 || x >= n || y >= n {
                None
            } else {
                self.reader
                    .get_tile(z as i64, x, n - 1 - y)
                    .ok()
                    .flatten()
                    .and_then(|data| png::decode(&data).ok())
                    .and_then(|png| terrarium_decode(&png))
            };
            // Cache misses too: all-ocean tiles simply don't exist.
            self.cache.insert(key, decoded);
            if self.cache.len() > 512 {
                self.cache.clear();
                self.cache.insert(key, None);
                return None;
            }
        }
        self.cache.get(&key).and_then(|t| t.as_ref())
    }

    /// Elevation (m) at web-mercator normalized coordinates; NaN outside
    /// the data coverage (renders transparent, not as fake sea).
    fn elevation_norm(&mut self, z: u32, nx: f64, ny: f64) -> f32 {
        let n = (1u64 << z) as f64;
        let fx = nx * n;
        let fy = ny * n;
        let tx = fx.floor();
        let ty = fy.floor();
        let px = ((fx - tx) * TILE_PX as f64 - 0.5).max(0.0);
        let py = ((fy - ty) * TILE_PX as f64 - 0.5).max(0.0);
        let (x0, y0) = (px.floor() as usize, py.floor() as usize);
        let (dx, dy) = ((px - x0 as f64) as f32, (py - y0 as f64) as f32);
        let x1 = (x0 + 1).min(TILE_PX - 1);
        let y1 = (y0 + 1).min(TILE_PX - 1);
        let Some(tile) = self.tile(z, tx as i64, ty as i64) else {
            return f32::NAN;
        };
        let at = |x: usize, y: usize| tile[y * TILE_PX + x];
        let top = at(x0, y0) * (1.0 - dx) + at(x1, y0) * dx;
        let bottom = at(x0, y1) * (1.0 - dx) + at(x1, y1) * dx;
        top * (1.0 - dy) + bottom * dy
    }

    /// Render hillshade+tint RGBA for a normalized-mercator bbox, plus the
    /// elevation grid (meters, row-major width*height; no-data = 0.0) that
    /// drives the renderer's 3D terrain displacement.
    pub fn shade_region(
        &mut self,
        norm_west: f64,
        norm_north: f64,
        norm_east: f64,
        norm_south: f64,
        width: usize,
        height: usize,
    ) -> (Vec<u8>, Vec<f32>, Vec<u8>) {
        // Zoom with ~1 source pixel per output pixel.
        let span = (norm_east - norm_west).max(1e-9);
        let want = (width as f64 / (span * TILE_PX as f64)).log2().ceil() as i64;
        let z = want.clamp(self.min_zoom as i64, self.max_zoom as i64) as u32;

        // Elevation grid with a 1px apron for gradients.
        let gw = width + 2;
        let gh = height + 2;
        let mut elev = vec![0f32; gw * gh];
        for gy in 0..gh {
            let ny = norm_north
                + (norm_south - norm_north) * ((gy as f64 - 1.0 + 0.5) / height as f64);
            for gx in 0..gw {
                let nx = norm_west
                    + (norm_east - norm_west) * ((gx as f64 - 1.0 + 0.5) / width as f64);
                elev[gy * gw + gx] =
                    self.elevation_norm(z, nx.clamp(0.0, 1.0), ny.clamp(0.0, 1.0));
            }
        }

        // Meters per output pixel (for gradient scaling): bbox spans
        // span*40075km of mercator "equator meters"; fine for shading.
        let m_per_px = (span * 40_075_016.0 / width as f64) as f32;
        // Same sun family as the building walls (one SceneSun).
        let (lx, ly, lz) = self.sun;
        let len = (lx * lx + ly * ly + lz * lz).sqrt();
        let (lx, ly, lz) = (lx / len, ly / len, lz / len);

        let mut out = vec![0u8; width * height * 4];
        let mut elev_out = vec![0f32; width * height];
        // Hillshade light factor (x255) per pixel, for draping landcover
        // colors with the same lighting as the hypsometric ramp.
        let mut shade_out = vec![0u8; width * height];
        for y in 0..height {
            for x in 0..width {
                let g = (y + 1) * gw + (x + 1);
                let e = elev[g];
                // No data (outside coverage) or open sea: transparent —
                // painting fake teal slabs at the coverage edge looked
                // broken.
                if e.is_nan() {
                    continue;
                }
                elev_out[y * width + x] = e.max(0.0);
                let e_e = if elev[g + 1].is_nan() { e } else { elev[g + 1] };
                let e_w = if elev[g - 1].is_nan() { e } else { elev[g - 1] };
                let e_s = if elev[g + gw].is_nan() { e } else { elev[g + gw] };
                let e_n = if elev[g - gw].is_nan() { e } else { elev[g - gw] };
                let gx = (e_e - e_w) / (2.0 * m_per_px);
                let gy = (e_s - e_n) / (2.0 * m_per_px);
                if e == 0.0 && gx == 0.0 && gy == 0.0 {
                    continue;
                }
                // Surface normal (y-down screen = south positive like the map)
                let inv = 1.0 / (1.0 + gx * gx + gy * gy).sqrt();
                let (nx, ny, nz) = (-gx * inv, -gy * inv, inv);
                let ndl = (nx * lx + ny * ly + nz * lz).max(0.0);
                let shade = 0.45 + 0.55 * ndl;
                shade_out[y * width + x] = (shade * 255.0).min(255.0) as u8;
                // Hypsometric ramp: NL polders green-blue, lowland
                // sand→brown, then Mittelgebirge→Alpine rock→snow.
                let (r, g_, b) = if e < -0.5 {
                    (96, 148, 130)
                } else if e < 5.0 {
                    (128, 168, 128)
                } else if e < 20.0 {
                    (160, 186, 130)
                } else if e < 60.0 {
                    (196, 192, 140)
                } else if e < 150.0 {
                    (206, 178, 125)
                } else if e < 400.0 {
                    (188, 148, 105)
                } else if e < 900.0 {
                    (162, 126, 92)
                } else if e < 1600.0 {
                    (138, 116, 96)
                } else if e < 2400.0 {
                    (148, 142, 136)
                } else {
                    (232, 236, 240)
                };
                let px = &mut out[(y * width + x) * 4..(y * width + x) * 4 + 4];
                px[0] = (r as f32 * shade).min(255.0) as u8;
                px[1] = (g_ as f32 * shade).min(255.0) as u8;
                px[2] = (b as f32 * shade).min(255.0) as u8;
                px[3] = 140;
            }
        }
        (out, elev_out, shade_out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn norm_of(lon: f64, lat: f64) -> (f64, f64) {
        let x = (lon + 180.0) / 360.0;
        let r = lat.to_radians();
        let y = (1.0 - (r.tan() + 1.0 / r.cos()).ln() / std::f64::consts::PI) / 2.0;
        (x, y)
    }

    #[test]
    #[ignore]
    fn probe_elevations() {
        let Ok(mut shader) =
            TerrainShader::open(Path::new("../../local/overlays/nl-terrain.mbtiles"))
        else {
            return;
        };
        for z in [6u32, 8, 10, 12] {
            for (name, lon, lat) in [
                ("adam", 4.9f64, 52.37f64),
                ("polder", 5.4, 52.5),
                ("veluwe", 5.83, 52.25),
                ("limburg", 5.97, 50.77),
                ("sea", 3.5, 53.0),
                ("germany", 7.8, 52.0),
            ] {
                let (nx, ny) = norm_of(lon, lat);
                let e = shader.elevation_norm(z, nx, ny);
                print!("z{z} {name}={e:.2} ");
            }
            println!();
        }
    }

    #[test]
    #[ignore]
    fn probe_wide_viewport() {
        let Ok(mut shader) =
            TerrainShader::open(Path::new("../../local/overlays/nl-terrain.mbtiles"))
        else {
            println!("OPEN FAILED");
            return;
        };
        // Same bbox the app requests at the zoomed-out verification view.
        let (cx, cy) = norm_of(5.5, 52.2);
        let (hw, hh) = (0.0432, 0.0324);
        let (rgba, _, _) = shader.shade_region(cx - hw, cy - hh, cx + hw, cy + hh, 1024, 768);
        let visible = rgba.chunks_exact(4).filter(|px| px[3] > 0).count();
        println!("wide probe: {visible} of {} px visible", 1024 * 768);
    }

    #[test]
    #[ignore]
    fn dump_shade_probes() {
        let Ok(mut shader) =
            TerrainShader::open(Path::new("../../local/overlays/nl-terrain.mbtiles"))
        else {
            return;
        };
        for (name, lon, lat, half_deg) in [
            ("vaals", 5.97, 50.77, 0.25),
            ("adam", 4.9, 52.37, 0.25),
            ("alps", 11.4, 47.27, 0.5),
            ("paris", 2.35, 48.85, 0.25),
        ] {
            let (cx, cy) = norm_of(lon, lat);
            let h = half_deg / 360.0;
            let (rgba, _, _) = shader.shade_region(cx - h, cy - h, cx + h, cy + h, 512, 512);
            let rgb: Vec<u8> = rgba
                .chunks_exact(4)
                .flat_map(|px| [px[0], px[1], px[2]])
                .collect();
            let png = crate::png::encode(512, 512, crate::png::PngFormat::Rgb8, &rgb);
            let out = format!(
                "/private/tmp/claude-501/-Users-admin-makepad-makepad/2360ceda-2c5b-45d8-91f7-072799a0d3d9/scratchpad/terrain_{name}.png"
            );
            std::fs::write(out, png).unwrap();
            let sample = &rgba[(256 * 512 + 256) * 4..(256 * 512 + 256) * 4 + 4];
            println!("{name}: center px {:?}", sample);
        }
    }
}
