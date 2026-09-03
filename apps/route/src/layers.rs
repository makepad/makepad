//! Map layer & theme state — parity with examples/map (route.md map.set_layer).
//!
//! Geodata overlay mbtiles, KNMI rain radar animation, GFS wind field,
//! terrain hillshade (shared drape from widgets), and the map themes.
//! Tools mutate `LayerState` and set `dirty`; the app applies the state to
//! the MapView after each tool run and feeds worker results as they arrive.

use makepad_widgets::*;
use crate::overlays::{OverlaySelection, OVERLAY_LAYERS};
use std::path::PathBuf;
use std::sync::mpsc;

pub struct WindUpdate {
    pub nx: usize,
    pub ny: usize,
    pub u: Vec<f32>,
    pub v: Vec<f32>,
    pub bbox: (f64, f64, f64, f64),
}

/// (normalized-mercator bbox, sun dir, cast_shadows)
pub type TerrainRequest = ((f64, f64, f64, f64), (f32, f32, f32), bool);

pub struct TerrainUpdate {
    pub texels: Vec<u32>,
    pub width: usize,
    pub height: usize,
    pub elev_texels: Vec<u32>,
    pub elev: Vec<f32>,
    pub elev_width: usize,
    pub elev_height: usize,
    pub bbox: (f64, f64, f64, f64),
}

pub struct LayerState {
    pub maps_root: PathBuf,
    pub overlays: OverlaySelection,
    pub rain: bool,
    pub wind: bool,
    pub terrain: bool,
    /// Tilt-shift blur over the map when tilted (gauss pyramid). On by default —
    /// it only becomes visible when the camera tilts (driving view).
    pub tilt_shift: bool,
    /// 0 light, 1 night, 2 circuit.
    pub theme: u32,
    /// Set by tools; the app applies + clears it after the tool run.
    pub dirty: bool,
    pub wind_worker_started: bool,
    pub wind_cache: Option<WindUpdate>,
    pub terrain_tx: Option<mpsc::Sender<TerrainRequest>>,
    pub last_terrain_key: Option<(i64, i64, i64, i64)>,
}

impl Default for LayerState {
    fn default() -> Self {
        Self {
            maps_root: PathBuf::new(),
            overlays: OverlaySelection::default(),
            rain: false,
            wind: false,
            terrain: false,
            tilt_shift: true,
            theme: 0,
            dirty: false,
            wind_worker_started: false,
            wind_cache: None,
            terrain_tx: None,
            last_terrain_key: None,
        }
    }
}

impl LayerState {
    pub fn set_maps_root(&mut self, maps_root: PathBuf) {
        self.maps_root = maps_root;
    }

    /// Toggle a layer by user-facing name. Returns the canonical name.
    pub fn set_layer(&mut self, name: &str, on: bool) -> Result<&'static str, String> {
        let key = name.trim().to_ascii_lowercase();
        self.dirty = true;
        match key.as_str() {
            "rain" | "radar" => {
                self.rain = on;
                Ok("rain")
            }
            "wind" => {
                self.wind = on;
                Ok("wind")
            }
            "terrain" | "hillshade" => {
                self.terrain = on;
                Ok("terrain")
            }
            "tiltshift" | "tilt_shift" | "tilt-shift" => {
                self.tilt_shift = on;
                Ok("tiltshift")
            }
            _ => {
                if let Some(layer_name) = self.overlays.set_named(&key, on) {
                    return Ok(layer_name);
                }
                self.dirty = false;
                Err(format!(
                    "unknown layer '{name}' — available: rain, wind, terrain, {}",
                    OVERLAY_LAYERS.map(|layer| layer.name).join(", ")
                ))
            }
        }
    }

    pub fn set_theme_name(&mut self, name: &str) -> Result<u32, String> {
        let theme = match name.trim().to_ascii_lowercase().as_str() {
            "light" | "day" | "default" => 0,
            "night" | "dark" => 1,
            "circuit" => 2,
            other => return Err(format!("unknown theme '{other}' — light, night or circuit")),
        };
        self.theme = theme;
        self.dirty = true;
        Ok(theme)
    }

    pub fn summary(&self) -> String {
        let mut on = self.overlays.enabled_names();
        if self.rain {
            on.push("rain");
        }
        if self.wind {
            on.push("wind");
        }
        if self.terrain {
            on.push("terrain");
        }
        if self.tilt_shift {
            on.push("tiltshift");
        }
        let theme = ["light", "night", "circuit"][self.theme.min(2) as usize];
        if on.is_empty() {
            format!("no layers active, theme {theme}")
        } else {
            format!("layers: {} | theme {theme}", on.join(", "))
        }
    }
}

/// GFS wind worker: 30 min disk-gated NOMADS polls, cached GRIB2 on disk.
pub fn start_wind_worker(spawner: ThreadSpawner, sender: ToUISender<WindUpdate>) {
    let spawned = spawner.spawn_worker(
        ThreadOptions {
            name: Some("route-wind".into()),
            ..Default::default()
        },
        move || {
        use makepad_geodata::wind::{WindSync, WIND_EAST, WIND_NORTH, WIND_SOUTH, WIND_WEST};
        let sync = WindSync::new("local/overlays/wind");
        let pacing = CancellationToken::new();
        loop {
            let field = sync.sync().ok().flatten().or_else(|| sync.cached());
            if let Some(field) = field {
                let _ = sender.send(WindUpdate {
                    nx: field.nx,
                    ny: field.ny,
                    u: field.u,
                    v: field.v,
                    bbox: (WIND_WEST, WIND_SOUTH, WIND_EAST, WIND_NORTH),
                });
            }
            let _ = pacing.wait_until(Cx::monotonic_now() + 300.0);
        }
    });
    match spawned {
        Ok(handle) => handle.detach(),
        Err(error) => log!("wind worker unavailable: {error}"),
    }
}

/// Terrain worker: hillshade renders for requested view bboxes from the
/// local terrarium mbtiles, with landcover drape (shared with examples/map).
pub fn start_terrain_worker(
    spawner: ThreadSpawner,
    sender: ToUISender<TerrainUpdate>,
    maps_root: PathBuf,
) -> mpsc::Sender<TerrainRequest> {
    let (tx, rx) = mpsc::channel::<TerrainRequest>();
    let spawned = spawner.spawn_worker(
        ThreadOptions {
            name: Some("route-terrain".into()),
            ..Default::default()
        },
        move || {
        use makepad_geodata::terrain_shade::TerrainShader;
        let Ok(mut shader) =
            TerrainShader::open(std::path::Path::new("local/overlays/nl-terrain.mbtiles"))
        else {
            return;
        };
        let mut land_reader = makepad_mbtile_reader::MbtilesReader::open(
            &maps_root.join("europe-shortbread.mbtiles"),
        )
        .ok();
        while let Ok(mut request) = rx.recv() {
            // Only the newest pending request matters.
            while let Ok(newer) = rx.try_recv() {
                request = newer;
            }
            let (bbox, sun, cast_shadows) = request;
            shader.sun = sun;
            shader.cast_shadows = cast_shadows;
            let (w, h) = (4096usize, 3072usize);
            let (mut rgba, elev_full, shade) =
                shader.shade_region(bbox.0, bbox.1, bbox.2, bbox.3, w, h);
            if let Some(reader) = land_reader.as_mut() {
                makepad_widgets::map::drape::drape_landcover(
                    reader, bbox, w, h, &elev_full, &shade, &mut rgba,
                );
            }
            let texels = makepad_geodata::radar_raster::rgba_to_bgra_texels(&rgba);
            // Displacement grid matching the renderer's terrain mesh corners
            // (same constants as examples/map).
            let (ew, eh) = (289usize, 217usize);
            let mut elev = vec![0f32; ew * eh];
            let mut elev_texels = vec![0u32; ew * eh];
            for y in 0..eh {
                for x in 0..ew {
                    let sx = (x * (w - 1)) / (ew - 1);
                    let sy = (y * (h - 1)) / (eh - 1);
                    let m = elev_full[sy * w + sx].max(0.0);
                    elev[y * ew + x] = m;
                    // Terrarium pack: m + 32768 in R*256 + G + B/256.
                    let v = ((m + 32768.0) * 256.0) as u32;
                    let (r, g, b) = (v >> 16 & 255, v >> 8 & 255, v & 255);
                    elev_texels[y * ew + x] = b | (g << 8) | (r << 16) | (255 << 24);
                }
            }
            let _ = sender.send(TerrainUpdate {
                texels,
                width: w,
                height: h,
                elev_texels,
                elev,
                elev_width: ew,
                elev_height: eh,
                bbox,
            });
        }
    });
    match spawned {
        Ok(handle) => handle.detach(),
        Err(error) => log!("terrain worker unavailable: {error}"),
    }
    tx
}

/// Terrain render request for the current viewport, debounced by view key
/// (same policy as examples/map: ~3 viewports coverage, half-viewport
/// re-render steps, integer zoom buckets).
pub fn request_terrain(cx: &mut Cx, map: &MapViewRef, state: &mut LayerState) {
    if !state.terrain || state.terrain_tx.is_none() {
        return;
    }
    let Some((lon, lat)) = map.center() else {
        return;
    };
    let zoom = map.map_zoom().unwrap_or(10.0);
    let tilt = map.tilt();
    let world_px = 256.0 * 2f64.powf(zoom);
    let margin = 3.0 + 1.6 * tilt.to_radians().sin();
    let half_w = 1000.0 * margin / world_px;
    let half_h = 750.0 * margin / world_px;
    let nx = (lon + 180.0) / 360.0;
    let lat_rad = lat.to_radians();
    let ny = (1.0 - (lat_rad.tan() + 1.0 / lat_rad.cos()).ln() / std::f64::consts::PI) / 2.0;
    let bbox = (nx - half_w, ny - half_h, nx + half_w, ny + half_h);
    let step = half_w / 3.0;
    let key = (
        (nx / step).round() as i64,
        (ny / step).round() as i64,
        zoom.floor() as i64,
        (tilt > 0.5) as i64,
    );
    if state.last_terrain_key == Some(key) {
        return;
    }
    state.last_terrain_key = Some(key);
    let shiny = map.shiny().unwrap_or_default();
    if let Some(tx) = &state.terrain_tx {
        let _ = tx.send((
            bbox,
            (shiny.sun.dir.x, shiny.sun.dir.y, shiny.sun.dir.z),
            shiny.terrain_shadows,
        ));
    }
    let _ = cx;
}
