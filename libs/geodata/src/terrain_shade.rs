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
use std::sync::Arc;

const TILE_PX: usize = 256;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TerrainTileKey {
    pub z: u32,
    pub x: i64,
    pub y: i64,
}

/// Elevation input behind one shader surface. Native development can read the
/// original MBTiles directly; production native and web jobs receive the
/// decoded tile payloads fetched by the shared `.mkmap` archive plane.
pub enum TerrainSource {
    LocalMbtiles(MbtilesReader),
    Archive {
        min_zoom: u32,
        max_zoom: u32,
        tiles: HashMap<TerrainTileKey, Option<Arc<[u8]>>>,
    },
}

impl TerrainSource {
    pub fn local_mbtiles(path: &Path) -> Result<Self, String> {
        MbtilesReader::open(path)
            .map(Self::LocalMbtiles)
            .map_err(|error| format!("open terrain: {error}"))
    }

    pub fn archive(
        min_zoom: u32,
        max_zoom: u32,
        tiles: HashMap<TerrainTileKey, Option<Arc<[u8]>>>,
    ) -> Self {
        Self::Archive {
            min_zoom,
            max_zoom,
            tiles,
        }
    }

    pub fn replace_archive_tiles(
        &mut self,
        min_zoom: u32,
        max_zoom: u32,
        tiles: HashMap<TerrainTileKey, Option<Arc<[u8]>>>,
    ) -> Result<(), String> {
        let Self::Archive {
            min_zoom: source_min,
            max_zoom: source_max,
            tiles: source_tiles,
        } = self
        else {
            return Err("terrain source kind changed".to_string());
        };
        *source_min = min_zoom;
        *source_max = max_zoom;
        *source_tiles = tiles;
        Ok(())
    }

    fn zoom_range(&mut self) -> (u32, u32) {
        match self {
            Self::LocalMbtiles(reader) => reader
                .get_metadata()
                .ok()
                .and_then(|metadata| {
                    let min = metadata.get("minzoom")?.parse::<u32>().ok()?;
                    let max = metadata.get("maxzoom")?.parse::<u32>().ok()?;
                    (min <= max).then_some((min, max))
                })
                .unwrap_or((6, 12)),
            Self::Archive {
                min_zoom,
                max_zoom,
                ..
            } => (*min_zoom, *max_zoom),
        }
    }

    fn tile_bytes(&mut self, key: TerrainTileKey) -> Option<Arc<[u8]>> {
        match self {
            Self::LocalMbtiles(reader) => {
                let n = 1i64 << key.z;
                reader
                    .get_tile(key.z as i64, key.x, n - 1 - key.y)
                    .ok()
                    .flatten()
                    .map(Arc::from)
            }
            Self::Archive { tiles, .. } => tiles.get(&key).cloned().flatten(),
        }
    }
}

/// Reused CPU render storage. Constructing it reserves the maximum render
/// dimensions, so viewport renders at or below that cap only change lengths
/// and clear bytes; they never allocate new backing buffers.
pub struct TerrainScratch {
    max_width: usize,
    max_height: usize,
    elevation_apron: Vec<f32>,
    rgba: Vec<u8>,
    elevation: Vec<f32>,
    shade: Vec<u8>,
    shadow: Vec<f32>,
}

impl TerrainScratch {
    pub fn with_capacity(max_width: usize, max_height: usize) -> Self {
        let pixels = max_width.saturating_mul(max_height);
        let apron = max_width
            .saturating_add(2)
            .saturating_mul(max_height.saturating_add(2));
        Self {
            max_width,
            max_height,
            elevation_apron: Vec::with_capacity(apron),
            rgba: Vec::with_capacity(pixels.saturating_mul(4)),
            elevation: Vec::with_capacity(pixels),
            shade: Vec::with_capacity(pixels),
            shadow: Vec::with_capacity(pixels),
        }
    }

    fn prepare(&mut self, width: usize, height: usize) {
        assert!(width <= self.max_width && height <= self.max_height);
        let pixels = width * height;
        self.elevation_apron.resize((width + 2) * (height + 2), 0.0);
        self.rgba.resize(pixels * 4, 0);
        self.elevation.resize(pixels, 0.0);
        self.shade.resize(pixels, 0);
        self.shadow.resize(pixels, 1.0);
        self.elevation_apron.fill(0.0);
        self.rgba.fill(0);
        self.elevation.fill(0.0);
        self.shade.fill(0);
        self.shadow.fill(1.0);
    }

    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }

    pub fn elevation(&self) -> &[f32] {
        &self.elevation
    }

    pub fn shade(&self) -> &[u8] {
        &self.shade
    }

    pub fn render_parts(&mut self) -> (&mut [u8], &[f32], &[u8]) {
        (&mut self.rgba, &self.elevation, &self.shade)
    }
}

fn render_zoom(span: f64, width: usize, min_zoom: u32, max_zoom: u32) -> u32 {
    let want = (width as f64 / (span.max(1e-9) * TILE_PX as f64))
        .log2()
        .ceil() as i64;
    want.clamp(min_zoom as i64, max_zoom as i64) as u32
}

/// Exact XYZ tile rectangle sampled by a render, including the one-pixel
/// gradient apron. Callers use this plan to range-fetch before submitting the
/// CPU job, so the shader never performs I/O on a wasm worker.
pub fn terrain_tile_plan(
    bbox: (f64, f64, f64, f64),
    width: usize,
    height: usize,
    min_zoom: u32,
    max_zoom: u32,
) -> Vec<TerrainTileKey> {
    let (west, north, east, south) = bbox;
    let z = render_zoom(east - west, width, min_zoom, max_zoom);
    let n = 1i64 << z;
    let apron_x = (east - west) * 0.5 / width.max(1) as f64;
    let apron_y = (south - north) * 0.5 / height.max(1) as f64;
    let x0 = (((west - apron_x).clamp(0.0, 1.0) * n as f64).floor() as i64)
        .clamp(0, n - 1);
    let x1 = (((east + apron_x).clamp(0.0, 1.0) * n as f64).floor() as i64)
        .clamp(0, n - 1);
    let y0 = (((north - apron_y).clamp(0.0, 1.0) * n as f64).floor() as i64)
        .clamp(0, n - 1);
    let y1 = (((south + apron_y).clamp(0.0, 1.0) * n as f64).floor() as i64)
        .clamp(0, n - 1);
    let mut keys = Vec::with_capacity(((x1 - x0 + 1) * (y1 - y0 + 1)) as usize);
    for y in y0..=y1 {
        for x in x0..=x1 {
            keys.push(TerrainTileKey { z, x, y });
        }
    }
    keys
}

pub struct TerrainShader {
    source: TerrainSource,
    min_zoom: u32,
    max_zoom: u32,
    /// Decoded elevation tiles (z, x, y) → 256x256 meters grid.
    cache: HashMap<(u32, i64, i64), Option<Vec<f32>>>,
    /// Sun direction (map space: x east, y south/screen-down, z up),
    /// normalized at use. Defaults to the legacy hillshade sun; set it from
    /// the app's `SceneSun` so the whole scene reads as one light source.
    pub sun: (f32, f32, f32),
    /// Horizon-march cast shadows (shiny.md T3): mountains shadow their
    /// valleys. Pure CPU on the elevation grid the shader already walks;
    /// skipped automatically when the view's relief is too small to show.
    pub cast_shadows: bool,
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
        Self::new(TerrainSource::local_mbtiles(path)?)
    }

    pub fn new(mut source: TerrainSource) -> Result<Self, String> {
        let (min_zoom, max_zoom) = source.zoom_range();
        if min_zoom > max_zoom || max_zoom > 30 {
            return Err("invalid terrain zoom range".to_string());
        }
        Ok(Self {
            source,
            min_zoom,
            max_zoom,
            cache: HashMap::new(),
            sun: (-0.5, -0.62, 0.6),
            cast_shadows: false,
        })
    }

    pub fn replace_archive_tiles(
        &mut self,
        min_zoom: u32,
        max_zoom: u32,
        tiles: HashMap<TerrainTileKey, Option<Arc<[u8]>>>,
    ) -> Result<(), String> {
        self.source
            .replace_archive_tiles(min_zoom, max_zoom, tiles)?;
        self.min_zoom = min_zoom;
        self.max_zoom = max_zoom;
        Ok(())
    }

    fn tile(&mut self, z: u32, x: i64, y: i64) -> Option<&Vec<f32>> {
        let key = (z, x, y);
        if !self.cache.contains_key(&key) {
            let n = 1i64 << z;
            let decoded = if x < 0 || y < 0 || x >= n || y >= n {
                None
            } else {
                if self.cache.len() >= 512 {
                    self.cache.clear();
                }
                self.source
                    .tile_bytes(TerrainTileKey { z, x, y })
                    .and_then(|data| png::decode(&data).ok())
                    .and_then(|png| terrarium_decode(&png))
            };
            // Cache misses too: all-ocean tiles simply don't exist.
            self.cache.insert(key, decoded);
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
        let mut scratch = TerrainScratch::with_capacity(width, height);
        self.shade_region_into(
            norm_west,
            norm_north,
            norm_east,
            norm_south,
            width,
            height,
            &mut scratch,
        );
        (
            scratch.rgba,
            scratch.elevation,
            scratch.shade,
        )
    }

    pub fn shade_region_into(
        &mut self,
        norm_west: f64,
        norm_north: f64,
        norm_east: f64,
        norm_south: f64,
        width: usize,
        height: usize,
        scratch: &mut TerrainScratch,
    ) {
        scratch.prepare(width, height);
        // Zoom with ~1 source pixel per output pixel.
        let span = (norm_east - norm_west).max(1e-9);
        let z = render_zoom(span, width, self.min_zoom, self.max_zoom);

        // Elevation grid with a 1px apron for gradients.
        let gw = width + 2;
        let gh = height + 2;
        let elev = &mut scratch.elevation_apron;
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

        // T3 terrain cast shadows: march each pixel toward the sun through
        // the elevation grid; terrain rising above the sun ray shadows it.
        // Exponential stride keeps it ~13 samples/px; only run when the
        // region has relief that could actually cast (skips the lowlands).
        let mut shadow_active = false;
        if self.cast_shadows {
            let (mut e_min, mut e_max) = (f32::MAX, f32::MIN);
            for &e in elev.iter() {
                if !e.is_nan() {
                    e_min = e_min.min(e);
                    e_max = e_max.max(e);
                }
            }
            let horiz = (lx * lx + ly * ly).sqrt();
            if e_max - e_min > 150.0 && horiz > 1e-4 {
                // Ray rise per meter of ground distance toward the sun.
                let sun_slope = lz / horiz;
                // March direction TOWARD the sun, in grid pixels.
                let (dx, dy) = (lx / horiz, ly / horiz);
                const STEPS: [f32; 17] = [
                    1.0, 2.0, 3.0, 4.0, 6.0, 8.0, 11.0, 15.0, 21.0, 29.0, 40.0, 56.0, 78.0,
                    109.0, 152.0, 213.0, 298.0,
                ];
                let dim = &mut scratch.shadow;
                for y in 0..height {
                    for x in 0..width {
                        let e0 = elev[(y + 1) * gw + (x + 1)];
                        if e0.is_nan() {
                            continue;
                        }
                        let mut occ = 0.0f32;
                        for step in STEPS {
                            let sx = x as f32 + dx * step;
                            let sy = y as f32 + dy * step;
                            if sx < 0.0 || sy < 0.0 || sx >= (width - 1) as f32
                                || sy >= (height - 1) as f32
                            {
                                break;
                            }
                            let e_s = elev[(sy as usize + 1) * gw + (sx as usize + 1)];
                            if e_s.is_nan() {
                                continue;
                            }
                            let dist_m = step * m_per_px;
                            let rise = e_s - e0 - sun_slope * dist_m;
                            if rise > 0.0 {
                                occ = occ.max(rise / dist_m);
                            }
                        }
                        if occ > 0.0 {
                            // ~3 degrees over the ray = full-depth shadow.
                            dim[y * width + x] = 1.0 - 0.45 * (occ / 0.05).min(1.0);
                        }
                    }
                }
                shadow_active = true;
            }
        }

        let out = &mut scratch.rgba;
        let elev_out = &mut scratch.elevation;
        // Hillshade light factor (x255) per pixel, for draping landcover
        // colors with the same lighting as the hypsometric ramp.
        let shade_out = &mut scratch.shade;
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
                let cast = if shadow_active {
                    scratch.shadow[y * width + x]
                } else {
                    1.0
                };
                let shade = (0.45 + 0.55 * ndl) * cast;
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
    }
}

#[cfg(test)]
// Native CPU benchmark assertions use std deadlines only inside tests.
#[allow(clippy::disallowed_types, clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn terrain_archive_source_reuses_scratch() {
        let mut rgb = Vec::with_capacity(TILE_PX * TILE_PX * 3);
        for _ in 0..TILE_PX * TILE_PX {
            // Terrarium encoding for 100 m: 128 * 256 + 100 - 32768.
            rgb.extend_from_slice(&[128, 100, 0]);
        }
        let bytes = crate::png::encode(
            TILE_PX as u32,
            TILE_PX as u32,
            crate::png::PngFormat::Rgb8,
            &rgb,
        );
        let mut tiles = HashMap::new();
        tiles.insert(TerrainTileKey { z: 0, x: 0, y: 0 }, Some(Arc::from(bytes)));
        let mut shader = TerrainShader::new(TerrainSource::archive(0, 0, tiles)).unwrap();
        let mut scratch = TerrainScratch::with_capacity(32, 24);
        let rgba_ptr = scratch.rgba.as_ptr();
        let elevation_ptr = scratch.elevation.as_ptr();
        shader.shade_region_into(0.25, 0.25, 0.75, 0.75, 32, 24, &mut scratch);
        assert!(scratch
            .elevation()
            .iter()
            .all(|elevation| (*elevation - 100.0).abs() < 0.01));
        shader.shade_region_into(0.25, 0.25, 0.75, 0.75, 32, 24, &mut scratch);
        assert_eq!(rgba_ptr, scratch.rgba.as_ptr());
        assert_eq!(elevation_ptr, scratch.elevation.as_ptr());
        assert_eq!(terrain_tile_plan((0.25, 0.25, 0.75, 0.75), 32, 24, 0, 0).len(), 1);
    }

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
    fn dump_alps_shadow_ab() {
        let Ok(mut shader) =
            TerrainShader::open(Path::new("../../local/overlays/nl-terrain.mbtiles"))
        else {
            println!("no archive");
            return;
        };
        let (cx, cy) = norm_of(11.4, 47.27);
        let h = 0.1 / 360.0;
        for (name, cast) in [("off", false), ("on", true)] {
            shader.cast_shadows = cast;
            let start = std::time::Instant::now();
            let (rgba, _, _) = shader.shade_region(cx - h, cy - h, cx + h, cy + h, 512, 512);
            println!("alps shadows {name}: {:.0} ms", start.elapsed().as_secs_f64() * 1000.0);
            let rgb: Vec<u8> = rgba
                .chunks_exact(4)
                .flat_map(|px| [px[0], px[1], px[2]])
                .collect();
            let png = crate::png::encode(512, 512, crate::png::PngFormat::Rgb8, &rgb);
            let out = format!(
                "/private/tmp/claude-501/-Users-admin-makepad-makepad/dc97c21e-85e9-41f6-a8d1-03d180e6bf12/scratchpad/alps_shadow_{name}.png"
            );
            std::fs::write(out, png).unwrap();
        }
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
