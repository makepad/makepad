//! Map layer & theme state — parity with examples/map (route.md map.set_layer).
//!
//! Geodata overlay mbtiles, KNMI rain radar animation, GFS wind field,
//! terrain hillshade (shared drape from widgets), and the map themes.
//! Tools mutate `LayerState` and set `dirty`; the app applies the state to
//! the MapView after each tool run and feeds worker results as they arrive.

use makepad_widgets::*;
use crate::overlays::{OverlaySelection, OVERLAY_LAYERS};
use std::path::PathBuf;

pub struct WindUpdate {
    pub nx: usize,
    pub ny: usize,
    pub u: Vec<f32>,
    pub v: Vec<f32>,
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
