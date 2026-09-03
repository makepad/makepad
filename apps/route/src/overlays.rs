use makepad_widgets::*;
use makepad_widgets::map::{
    archive::{new_archive_worker_pool, MapTileArchive, TileBytesResult},
    geometry::TileKey,
};
use makepad_geodata::terrain_shade::{
    terrain_tile_plan, TerrainScratch, TerrainShader, TerrainSource, TerrainTileKey,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub const OVERLAY_COUNT: usize = 6;
pub const TERRAIN_LOCAL_MBTILES: &str = "local/overlays/nl-terrain.mbtiles";
pub const TERRAIN_HOSTED_URL: &str =
    "https://makepad.nl/maps/overlays/terrain-20260903.mkmap/";
const TERRAIN_ARCHIVE_GENERATION: u64 = 1;
const TERRAIN_FETCH_WINDOW: usize = 48;
const TERRAIN_MAX_WIDTH: usize = 2048;
const TERRAIN_MAX_HEIGHT: usize = 1536;
const TERRAIN_ELEV_WIDTH: usize = 289;
const TERRAIN_ELEV_HEIGHT: usize = 217;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OverlayLayer {
    pub name: &'static str,
    pub local_mbtiles: &'static str,
    pub option: Option<&'static str>,
}

/// One ordered layer table drives native tools, both checkbox controllers,
/// and the provisioners' source tables.
pub const OVERLAY_LAYERS: [OverlayLayer; OVERLAY_COUNT] = [
    OverlayLayer {
        name: "chargers",
        local_mbtiles: "local/overlays/nl-chargers.mbtiles",
        option: Some("fast"),
    },
    OverlayLayer {
        name: "transit",
        local_mbtiles: "local/overlays/nl-transit.mbtiles",
        option: None,
    },
    OverlayLayer {
        name: "nature",
        local_mbtiles: "local/overlays/nl-nature.mbtiles",
        option: None,
    },
    OverlayLayer {
        name: "districts",
        local_mbtiles: "local/overlays/nl-wijkbuurt.mbtiles",
        option: None,
    },
    OverlayLayer {
        name: "buildings_age",
        local_mbtiles: "local/overlays/nl-buildings-age.mbtiles",
        option: None,
    },
    OverlayLayer {
        name: "demographics",
        local_mbtiles: "local/overlays/nl-demographics.mbtiles",
        option: None,
    },
];

#[derive(Default)]
pub struct OverlaySelection {
    pub on: [bool; OVERLAY_COUNT],
}

impl OverlaySelection {
    pub fn set_named(&mut self, name: &str, on: bool) -> Option<&'static str> {
        let key = name.trim().to_ascii_lowercase();
        for (index, layer) in OVERLAY_LAYERS.iter().enumerate() {
            if layer.name == key
                || (key == "wijkbuurt" && layer.name == "districts")
                || (key == "buildings-age" && layer.name == "buildings_age")
            {
                self.on[index] = on;
                return Some(layer.name);
            }
        }
        None
    }

    pub fn enabled_sources(&self, available: &[OverlaySource]) -> Vec<OverlaySource> {
        available
            .iter()
            .zip(self.on.iter())
            .filter(|(_, on)| **on)
            .map(|(source, _)| source.clone())
            .collect()
    }

    pub fn enabled_names(&self) -> Vec<&'static str> {
        OVERLAY_LAYERS
            .iter()
            .zip(self.on.iter())
            .filter(|(_, on)| **on)
            .map(|(layer, _)| layer.name)
            .collect()
    }
}

pub fn overlay_source(layer: OverlayLayer, source: TileSourceConfig) -> OverlaySource {
    OverlaySource::with_option(layer.name, source, layer.option)
}

/// One source decision for both controllers. Native keeps the original
/// MBTiles as a development override; every other case uses the hosted
/// range-fetched archive, including wasm.
pub fn terrain_source() -> TileSourceConfig {
    #[cfg(feature = "native")]
    if Path::new(TERRAIN_LOCAL_MBTILES).is_file() {
        return TileSourceConfig::local_archive(TERRAIN_LOCAL_MBTILES);
    }
    TileSourceConfig::http_archive(TERRAIN_HOSTED_URL)
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TerrainWorkerSource {
    LocalMbtiles(String),
    Archive(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TerrainWorkerSpec {
    source: TerrainWorkerSource,
    landcover_path: Option<String>,
    max_width: usize,
    max_height: usize,
}

enum TerrainJobTiles {
    LocalMbtiles,
    Archive {
        min_zoom: u32,
        max_zoom: u32,
        tiles: HashMap<TerrainTileKey, Option<Arc<[u8]>>>,
    },
}

struct TerrainRenderRequest {
    id: u64,
    bbox: (f64, f64, f64, f64),
    sun: (f32, f32, f32),
    cast_shadows: bool,
    width: usize,
    height: usize,
    max_width: usize,
    max_height: usize,
    min_zoom: u32,
    max_zoom: u32,
    keys: Vec<TerrainTileKey>,
}

struct PendingTerrainFetch {
    request: TerrainRenderRequest,
    queued: VecDeque<TileKey>,
    loading: HashSet<TileKey>,
}

struct TerrainJobResult {
    id: u64,
    result: Result<TerrainOverlayData, String>,
}

struct TerrainWorker {
    spec: TerrainWorkerSpec,
    shader: TerrainShader,
    scratch: TerrainScratch,
    #[cfg(feature = "native")]
    landcover: Option<makepad_mbtile_reader::MbtilesReader>,
}

impl TerrainWorker {
    fn new(spec: TerrainWorkerSpec, tiles: &mut TerrainJobTiles) -> Result<Self, String> {
        let source = match (&spec.source, tiles) {
            (TerrainWorkerSource::LocalMbtiles(path), TerrainJobTiles::LocalMbtiles) => {
                TerrainSource::local_mbtiles(Path::new(path))?
            }
            (
                TerrainWorkerSource::Archive(_),
                TerrainJobTiles::Archive {
                    min_zoom,
                    max_zoom,
                    ..
                },
            ) => TerrainSource::archive(*min_zoom, *max_zoom, HashMap::new()),
            _ => return Err("terrain worker/source mismatch".to_string()),
        };
        let shader = TerrainShader::new(source)?;
        #[cfg(feature = "native")]
        let landcover = spec.landcover_path.as_deref().and_then(|path| {
            makepad_mbtile_reader::MbtilesReader::open(Path::new(path)).ok()
        });
        Ok(Self {
            scratch: TerrainScratch::with_capacity(spec.max_width, spec.max_height),
            shader,
            spec,
            #[cfg(feature = "native")]
            landcover,
        })
    }

    fn render(
        &mut self,
        request: TerrainRenderRequest,
        mut tiles: TerrainJobTiles,
    ) -> Result<TerrainOverlayData, String> {
        if let TerrainJobTiles::Archive {
            min_zoom,
            max_zoom,
            tiles,
        } = &mut tiles
        {
            self.shader
                .replace_archive_tiles(*min_zoom, *max_zoom, std::mem::take(tiles))?;
        }
        self.shader.sun = request.sun;
        self.shader.cast_shadows = request.cast_shadows;
        let (west, north, east, south) = request.bbox;
        self.shader.shade_region_into(
            west,
            north,
            east,
            south,
            request.width,
            request.height,
            &mut self.scratch,
        );

        #[cfg(feature = "native")]
        if let Some(reader) = self.landcover.as_mut() {
            let (rgba, elevation, shade) = self.scratch.render_parts();
            makepad_widgets::map::drape::drape_landcover(
                reader,
                request.bbox,
                request.width,
                request.height,
                elevation,
                shade,
                rgba,
            );
        }

        let texels = makepad_geodata::radar_raster::rgba_to_bgra_texels(self.scratch.rgba());
        let mut elev = vec![0.0; TERRAIN_ELEV_WIDTH * TERRAIN_ELEV_HEIGHT];
        let mut elev_texels = vec![0; TERRAIN_ELEV_WIDTH * TERRAIN_ELEV_HEIGHT];
        for y in 0..TERRAIN_ELEV_HEIGHT {
            for x in 0..TERRAIN_ELEV_WIDTH {
                let sx = (x * (request.width - 1)) / (TERRAIN_ELEV_WIDTH - 1);
                let sy = (y * (request.height - 1)) / (TERRAIN_ELEV_HEIGHT - 1);
                let meters = self.scratch.elevation()[sy * request.width + sx].max(0.0);
                let index = y * TERRAIN_ELEV_WIDTH + x;
                elev[index] = meters;
                let packed = ((meters + 32768.0) * 256.0) as u32;
                let (r, g, b) = (packed >> 16 & 255, packed >> 8 & 255, packed & 255);
                elev_texels[index] = b | (g << 8) | (r << 16) | (255 << 24);
            }
        }
        Ok(TerrainOverlayData {
            texels,
            width: request.width,
            height: request.height,
            elev_texels,
            elev,
            elev_width: TERRAIN_ELEV_WIDTH,
            elev_height: TERRAIN_ELEV_HEIGHT,
            bbox: request.bbox,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TerrainViewKey {
    x: i64,
    y: i64,
    zoom: i64,
    tilted: bool,
    width: usize,
    height: usize,
    sun: [u32; 3],
    cast_shadows: bool,
}

enum TerrainInput {
    LocalMbtiles { path: String },
    Archive { identity: String, archive: MapTileArchive },
}

/// UI-owned terrain orchestration. Archive networking stays on the UI event
/// path; decoded tile payloads cross to a single Heavy-lane CPU job. The
/// mutex is exclusively acquired by those serialized jobs, never by the UI.
pub struct TerrainLayer {
    enabled: bool,
    input: Option<TerrainInput>,
    tile_cache: HashMap<TileKey, Option<Arc<[u8]>>>,
    pending: Option<PendingTerrainFetch>,
    last_view: Option<TerrainViewKey>,
    next_request_id: u64,
    latest_request_id: u64,
    render_queue: TaskQueue<()>,
    worker: Arc<Mutex<Option<TerrainWorker>>>,
    worker_latest_request: Arc<AtomicU64>,
    results: ToUIReceiver<TerrainJobResult>,
    landcover_path: Option<String>,
}

impl Default for TerrainLayer {
    fn default() -> Self {
        Self {
            enabled: false,
            input: None,
            tile_cache: HashMap::new(),
            pending: None,
            last_view: None,
            next_request_id: 1,
            latest_request_id: 0,
            render_queue: TaskQueue::new(Lane::Heavy, 1, 1),
            worker: Arc::new(Mutex::new(None)),
            worker_latest_request: Arc::new(AtomicU64::new(0)),
            results: Default::default(),
            landcover_path: None,
        }
    }
}

impl TerrainLayer {
    pub fn set_enabled(
        &mut self,
        cx: &mut Cx,
        map: &MapViewRef,
        enabled: bool,
        maps_root: Option<&Path>,
    ) {
        #[cfg(feature = "native")]
        let landcover_path = maps_root.map(|root| {
            root.join("europe-shortbread.mbtiles")
                .to_string_lossy()
                .into_owned()
        });
        #[cfg(not(feature = "native"))]
        let landcover_path = {
            let _ = maps_root;
            None
        };
        if self.enabled == enabled && (!enabled || self.input.is_some()) {
            return;
        }
        self.enabled = enabled;
        self.landcover_path = landcover_path;
        self.last_view = None;
        self.latest_request_id = self.next_request_id;
        self.worker_latest_request
            .store(self.latest_request_id, Ordering::Release);
        self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
        self.render_queue.clear();
        self.cancel_pending(cx);
        if !enabled {
            map.set_terrain_overlay(cx, TerrainOverlayData::default());
            return;
        }
        if self.input.is_none() {
            self.install_source(cx, terrain_source());
        }
        self.request(cx, map);
    }

    fn install_source(&mut self, cx: &mut Cx, source: TileSourceConfig) {
        let workers = new_archive_worker_pool(cx);
        self.input = Some(match source {
            TileSourceConfig::LocalArchive { mbtiles_path, .. }
                if mbtiles_path.ends_with(".mbtiles") =>
            {
                TerrainInput::LocalMbtiles { path: mbtiles_path }
            }
            TileSourceConfig::LocalArchive { mbtiles_path, .. } => TerrainInput::Archive {
                identity: mbtiles_path.clone(),
                archive: MapTileArchive::file(mbtiles_path, workers),
            },
            TileSourceConfig::HttpArchive { root_url, .. } => TerrainInput::Archive {
                identity: root_url.clone(),
                archive: MapTileArchive::http(root_url, workers),
            },
        });
    }

    pub fn request(&mut self, cx: &mut Cx, map: &MapViewRef) {
        if !self.enabled {
            return;
        }
        let Some((lon, lat)) = map.center() else {
            return;
        };
        let rect = map.area().rect(cx);
        let logical_width = if rect.size.x > 0.0 { rect.size.x } else { 2000.0 };
        let logical_height = if rect.size.y > 0.0 { rect.size.y } else { 1500.0 };
        #[cfg(target_arch = "wasm32")]
        let dpr = 1.0;
        #[cfg(not(target_arch = "wasm32"))]
        let dpr = cx.get_dpi_factor_of(&map.area()).max(1.0);
        // Web is deliberately DPR 1. Native preserves the old 4096x3072
        // ceiling even on a 1x 4K display, while still scaling for HiDPI.
        #[cfg(target_arch = "wasm32")]
        let native_cap_scale = 1.0;
        #[cfg(not(target_arch = "wasm32"))]
        let native_cap_scale = dpr.max(2.0);
        let max_width = (TERRAIN_MAX_WIDTH as f64 * native_cap_scale).round() as usize;
        let max_height = (TERRAIN_MAX_HEIGHT as f64 * native_cap_scale).round() as usize;
        let width_limit = (logical_width * dpr)
            .round()
            .clamp(1.0, max_width as f64);
        let height_limit = (logical_height * dpr)
            .round()
            .clamp(1.0, max_height as f64);
        // The established terrain request is 4:3. Preserve that sampling
        // ratio while fitting inside the viewport-sized pixel budget.
        let render_scale = (width_limit / 4.0).min(height_limit / 3.0).max(0.25);
        let width = (render_scale * 4.0).round().max(1.0) as usize;
        let height = (render_scale * 3.0).round().max(1.0) as usize;

        let zoom = map.map_zoom().unwrap_or(10.0);
        let tilt = map.tilt();
        let world_px = 256.0 * 2f64.powf(zoom);
        let margin = 3.0 + 1.6 * tilt.to_radians().sin();
        let half_w = 1000.0 * margin / world_px;
        let half_h = 750.0 * margin / world_px;
        let nx = (lon + 180.0) / 360.0;
        let lat_rad = lat.to_radians();
        let ny = (1.0
            - (lat_rad.tan() + 1.0 / lat_rad.cos()).ln() / std::f64::consts::PI)
            / 2.0;
        let bbox = (nx - half_w, ny - half_h, nx + half_w, ny + half_h);
        let shiny = map.shiny().unwrap_or_default();
        let sun = (shiny.sun.dir.x, shiny.sun.dir.y, shiny.sun.dir.z);
        let step = half_w.max(1e-12) / 3.0;
        let view_key = TerrainViewKey {
            x: (nx / step).round() as i64,
            y: (ny / step).round() as i64,
            zoom: zoom.floor() as i64,
            tilted: tilt > 0.5,
            width,
            height,
            sun: [sun.0.to_bits(), sun.1.to_bits(), sun.2.to_bits()],
            cast_shadows: shiny.terrain_shadows,
        };
        if self.last_view.as_ref() == Some(&view_key) {
            return;
        }
        self.last_view = Some(view_key);
        self.cancel_pending(cx);
        let id = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
        self.latest_request_id = id;
        self.worker_latest_request.store(id, Ordering::Release);
        let (min_zoom, max_zoom) = match self.input.as_ref() {
            Some(TerrainInput::Archive { archive, .. }) => archive.zoom_range().unwrap_or((6, 12)),
            _ => (6, 12),
        };
        let keys = terrain_tile_plan(bbox, width, height, min_zoom, max_zoom);
        let request = TerrainRenderRequest {
            id,
            bbox,
            sun,
            cast_shadows: shiny.terrain_shadows,
            width,
            height,
            max_width,
            max_height,
            min_zoom,
            max_zoom,
            keys,
        };
        match self.input.as_ref() {
            Some(TerrainInput::LocalMbtiles { .. }) => {
                self.submit_render(cx, request, TerrainJobTiles::LocalMbtiles)
            }
            Some(TerrainInput::Archive { .. }) => self.begin_archive_fetch(cx, request),
            None => {}
        }
    }

    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event, map: &MapViewRef) {
        let archive_results = match self.input.as_mut() {
            Some(TerrainInput::Archive { archive, .. }) => archive.drain(cx, event),
            _ => Vec::new(),
        };
        for tile in archive_results {
            let bytes = match tile.result {
                TileBytesResult::Bytes(bytes) => Some(bytes),
                TileBytesResult::Missing => None,
                TileBytesResult::Error(error) => {
                    log!("terrain archive tile failed: {error}");
                    None
                }
            };
            self.tile_cache.insert(tile.key, bytes);
            if let Some(pending) = self.pending.as_mut() {
                pending.loading.remove(&tile.key);
            }
        }
        self.pump_archive_fetches(cx);
        self.finish_archive_fetch(cx);
        self.render_queue.pump(&cx.task_pool());
        while let Ok(result) = self.results.try_recv() {
            if self.enabled && result.id == self.latest_request_id {
                match result.result {
                    Ok(data) => map.set_terrain_overlay(cx, data),
                    Err(error) => log!("terrain render failed: {error}"),
                }
            }
        }
    }

    fn cancel_pending(&mut self, cx: &mut Cx) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        if let Some(TerrainInput::Archive { archive, .. }) = self.input.as_mut() {
            for key in pending.loading {
                archive.cancel_tile(cx, key);
            }
        }
    }

    fn begin_archive_fetch(&mut self, cx: &mut Cx, request: TerrainRenderRequest) {
        let wanted = request
            .keys
            .iter()
            .map(|key| TileKey {
                z: key.z,
                x: key.x as i32,
                y: key.y as i32,
            })
            .collect::<HashSet<_>>();
        if self.tile_cache.len() > 512 {
            self.tile_cache.retain(|key, _| wanted.contains(key));
        }
        let queued = wanted
            .into_iter()
            .filter(|key| !self.tile_cache.contains_key(key))
            .collect::<VecDeque<_>>();
        self.pending = Some(PendingTerrainFetch {
            request,
            queued,
            loading: HashSet::new(),
        });
        self.pump_archive_fetches(cx);
        self.finish_archive_fetch(cx);
    }

    fn pump_archive_fetches(&mut self, cx: &mut Cx) {
        let (Some(TerrainInput::Archive { archive, .. }), Some(pending)) =
            (self.input.as_mut(), self.pending.as_mut())
        else {
            return;
        };
        while pending.loading.len() < TERRAIN_FETCH_WINDOW {
            let Some(key) = pending.queued.pop_front() else {
                break;
            };
            pending.loading.insert(key);
            archive.request_tile(cx, key, TERRAIN_ARCHIVE_GENERATION, 0);
        }
        archive.flush(cx);
    }

    fn finish_archive_fetch(&mut self, cx: &mut Cx) {
        if !self.pending.as_ref().is_some_and(|pending| {
            pending.queued.is_empty() && pending.loading.is_empty()
        }) {
            return;
        }
        let pending = self.pending.take().unwrap();
        let tiles = pending
            .request
            .keys
            .iter()
            .map(|key| {
                let archive_key = TileKey {
                    z: key.z,
                    x: key.x as i32,
                    y: key.y as i32,
                };
                (*key, self.tile_cache.get(&archive_key).cloned().unwrap_or(None))
            })
            .collect();
        let min_zoom = pending.request.min_zoom;
        let max_zoom = pending.request.max_zoom;
        self.submit_render(
            cx,
            pending.request,
            TerrainJobTiles::Archive {
                min_zoom,
                max_zoom,
                tiles,
            },
        );
    }

    fn submit_render(
        &mut self,
        cx: &mut Cx,
        request: TerrainRenderRequest,
        mut tiles: TerrainJobTiles,
    ) {
        let source = match self.input.as_ref() {
            Some(TerrainInput::LocalMbtiles { path }) => {
                TerrainWorkerSource::LocalMbtiles(path.clone())
            }
            Some(TerrainInput::Archive { identity, .. }) => {
                TerrainWorkerSource::Archive(identity.clone())
            }
            None => return,
        };
        let spec = TerrainWorkerSpec {
            source,
            landcover_path: self.landcover_path.clone(),
            max_width: request.max_width,
            max_height: request.max_height,
        };
        let id = request.id;
        let worker = self.worker.clone();
        let worker_latest_request = self.worker_latest_request.clone();
        let sender = self.results.sender();
        let pool = cx.task_pool();
        if let Err(error) = self.render_queue.push(
            &pool,
            (),
            true,
            QueueOrder::Lifo,
            move || {
                // A pool-saturated job may already have crossed the staging
                // boundary. If a newer viewport arrived before it actually
                // began, supersede it without touching the reusable worker.
                if worker_latest_request.load(Ordering::Acquire) != id {
                    return;
                }
                let result = worker
                    .lock()
                    .map_err(|_| "terrain worker state poisoned".to_string())
                    .and_then(|mut slot| {
                        if slot.as_ref().is_none_or(|current| current.spec != spec) {
                            *slot = Some(TerrainWorker::new(spec, &mut tiles)?);
                        }
                        slot.as_mut().unwrap().render(request, tiles)
                    });
                let _ = sender.send(TerrainJobResult { id, result });
            },
        ) {
            log!("terrain render submission failed: {error}");
        }
    }
}

fn checkbox_ids() -> [&'static [LiveId]; OVERLAY_COUNT] {
    [
        ids!(layer_chargers),
        ids!(layer_transit),
        ids!(layer_nature),
        ids!(layer_districts),
        ids!(layer_buildings),
        ids!(layer_demographics),
    ]
}

pub fn sync_checkboxes(cx: &mut Cx, ui: &WidgetRef, selection: &OverlaySelection) {
    for (id, on) in checkbox_ids().into_iter().zip(selection.on) {
        ui.check_box(cx, id).set_active(cx, on, Animate::No);
    }
}

pub fn handle_checkboxes(
    cx: &mut Cx,
    ui: &WidgetRef,
    actions: &Actions,
    selection: &mut OverlaySelection,
) -> bool {
    let mut changed = false;
    for (index, id) in checkbox_ids().into_iter().enumerate() {
        if let Some(on) = ui.check_box(cx, id).changed(actions) {
            selection.on[index] = on;
            changed = true;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_aliases_share_the_ordered_layer_table() {
        let mut selection = OverlaySelection::default();
        assert_eq!(selection.set_named("wijkbuurt", true), Some("districts"));
        assert_eq!(selection.set_named("buildings-age", true), Some("buildings_age"));
        assert!(selection.on[3]);
        assert!(selection.on[4]);
    }

    #[test]
    fn charger_filter_stays_on_its_source() {
        let source = overlay_source(
            OVERLAY_LAYERS[0],
            TileSourceConfig::http_archive("https://makepad.nl/maps/chargers.mkmap/"),
        );
        assert_eq!(source.name, "chargers");
        let TileSourceConfig::HttpArchive { root_url, .. } = source.source else {
            panic!("hosted overlay changed source kind");
        };
        assert_eq!(root_url, "https://makepad.nl/maps/chargers.mkmap/?fast");
    }
}
