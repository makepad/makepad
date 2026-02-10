use crate::makepad_draw::vector::{
    append_tessellated_geometry, tessellate_path_fill, tessellate_path_stroke, LineCap, LineJoin,
    Tessellator, VVertex, VectorPath, VectorRenderParams, VECTOR_ZBIAS_STEP,
};
use crate::makepad_platform::makepad_micro_serde::*;
use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*, WidgetMatchEvent};
use flate2::read::{GzDecoder, ZlibDecoder};
use makepad_mbtile_reader::MbtilesReader;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

const TILE_SIZE: f64 = 256.0;
const OVERPASS_ENDPOINTS: &[&str] = &["https://overpass.kumi.systems/api/interpreter"];
const MAX_PENDING_REQUESTS: usize = 2;
const MAX_TILE_RETRIES: u8 = 6;
const RETRY_BASE_FRAMES: u64 = 30;
const RETRY_MAX_FRAMES: u64 = 300;
const TILE_CACHE_DIR: &str = "local/tilecache_v2";
const TILE_QUERY_PAD: f64 = 0.05;
const LOCAL_MBTILES_PATH: &str = "local/noord-holland-shortbread-1.0.mbtiles";
const LOCAL_MBTILES_MIN_ZOOM: u32 = 0;
const LOCAL_MBTILES_MAX_ZOOM: u32 = 14;
const MAX_LOCAL_TILE_BATCH: usize = 10;
const LABEL_COLLISION_PADDING: f64 = 4.0;
const LABEL_VIEW_MARGIN: f64 = 72.0;
const LABEL_MIN_PATH_PIXELS: f64 = 24.0;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.draw
    use mod.geom
    use mod.math
    use mod.shader

    mod.draw.DrawMapVector = mod.std.set_type_default() do #(DrawMapVector::script_shader(vm)){
        ..mod.draw.DrawVector

        vertex: fn() {
            let pos = vec2(self.geom.x, self.geom.y);
            let transformed = pos * self.map_scale + self.map_offset;

            self.v_tcoord = vec2(self.geom.u, self.geom.v);
            self.v_color = vec4(self.geom.color_r, self.geom.color_g, self.geom.color_b, self.geom.color_a);
            self.v_stroke_mult = self.geom.stroke_mult;
            self.v_stroke_dist = self.geom.stroke_dist;
            self.v_shape_id = self.geom.shape_id;
            self.v_param0 = self.geom.param0;
            self.v_param5 = self.geom.param5;

            let grad_type = self.geom.param0;
            if grad_type > 0.5 && grad_type < 1.5 {
                let p0 = vec2(self.geom.param1, self.geom.param2) * self.map_scale + self.map_offset;
                let p1 = vec2(self.geom.param3, self.geom.param4) * self.map_scale + self.map_offset;
                self.v_param1 = p0.x;
                self.v_param2 = p0.y;
                self.v_param3 = p1.x;
                self.v_param4 = p1.y;
            } else if grad_type > 1.5 {
                let center = vec2(self.geom.param1, self.geom.param2) * self.map_scale + self.map_offset;
                self.v_param1 = center.x;
                self.v_param2 = center.y;
                self.v_param3 = self.geom.param3 * self.map_scale.x;
                self.v_param4 = self.geom.param4 * self.map_scale.y;
            } else if self.geom.shape_id > 0.5 {
                let bbox_min = vec2(self.geom.param1, self.geom.param2) * self.map_scale + self.map_offset;
                let bbox_max = vec2(self.geom.param3, self.geom.param4) * self.map_scale + self.map_offset;
                self.v_param1 = bbox_min.x;
                self.v_param2 = bbox_min.y;
                self.v_param3 = bbox_max.x;
                self.v_param4 = bbox_max.y;
            } else {
                self.v_param1 = self.geom.param1;
                self.v_param2 = self.geom.param2;
                self.v_param3 = self.geom.param3;
                self.v_param4 = self.geom.param4;
            }

            let shifted = transformed + self.draw_list.view_shift;
            self.v_world = shifted;

            let cr = self.geom.clip_radius * max(self.map_scale.x, self.map_scale.y);
            let clip = vec4(
                max(self.draw_clip.x, self.draw_list.view_clip.x - self.draw_list.view_shift.x),
                max(self.draw_clip.y, self.draw_list.view_clip.y - self.draw_list.view_shift.y),
                min(self.draw_clip.z, self.draw_list.view_clip.z - self.draw_list.view_shift.x),
                min(self.draw_clip.w, self.draw_list.view_clip.w - self.draw_list.view_shift.y)
            );

            if transformed.x + cr < clip.x || transformed.y + cr < clip.y
                || transformed.x - cr > clip.z || transformed.y - cr > clip.w {
                self.vertex_pos = vec4(0.0, 0.0, 0.0, 0.0);
                return
            }

            let world = self.draw_list.view_transform * vec4(
                shifted.x
                shifted.y
                self.draw_depth + self.draw_call.zbias + self.geom.zbias
                1.
            );
            self.vertex_pos = self.draw_pass.camera_projection * (self.draw_pass.camera_view * world)
        }
    }

    mod.draw.DrawRotatedText = mod.std.set_type_default() do #(DrawRotatedText::script_shader(vm)){
        ..mod.draw.DrawText

        rotated_pos: varying(vec2f)

        vertex: fn() {
            let p = mix(self.rect_pos, self.rect_pos + self.rect_size, self.geom.pos)
            let origin = self.rotation_origin
            let scaled = (p - origin) * self.label_scale
            let cs = cos(self.rotation)
            let sn = sin(self.rotation)
            let rotated = vec2(
                scaled.x * cs - scaled.y * sn,
                scaled.x * sn + scaled.y * cs
            ) + origin

            self.pos = self.geom.pos
            self.t = mix(self.t_min, self.t_max, self.geom.pos.xy)
            self.rotated_pos = rotated

            let half_extent = self.rect_size * self.label_scale * 0.5
            let cr = length(half_extent) + 2.0
            let clip = vec4(
                max(self.draw_clip.x, self.draw_list.view_clip.x - self.draw_list.view_shift.x),
                max(self.draw_clip.y, self.draw_list.view_clip.y - self.draw_list.view_shift.y),
                min(self.draw_clip.z, self.draw_list.view_clip.z - self.draw_list.view_shift.x),
                min(self.draw_clip.w, self.draw_list.view_clip.w - self.draw_list.view_shift.y)
            )

            if rotated.x + cr < clip.x || rotated.y + cr < clip.y
                || rotated.x - cr > clip.z || rotated.y - cr > clip.w {
                self.vertex_pos = vec4(0.0, 0.0, 0.0, 0.0)
                return
            }

            let shifted = rotated + self.draw_list.view_shift
            self.world = self.draw_list.view_transform * vec4(
                shifted.x,
                shifted.y,
                self.glyph_depth + self.draw_call.zbias,
                1.
            )
            self.vertex_pos = self.draw_pass.camera_projection * (self.draw_pass.camera_view * self.world)
        }

        pixel: fn() {
            let clip = vec4(
                max(self.draw_clip.x, self.draw_list.view_clip.x - self.draw_list.view_shift.x),
                max(self.draw_clip.y, self.draw_list.view_clip.y - self.draw_list.view_shift.y),
                min(self.draw_clip.z, self.draw_list.view_clip.z - self.draw_list.view_shift.x),
                min(self.draw_clip.w, self.draw_list.view_clip.w - self.draw_list.view_shift.y)
            )
            if self.rotated_pos.x < clip.x || self.rotated_pos.y < clip.y
                || self.rotated_pos.x > clip.z || self.rotated_pos.y > clip.w {
                discard()
            }
            return self.sample_text_pixel()
        }
    }

    mod.widgets.MapViewBase = #(MapView::register_widget(vm))

    mod.widgets.MapView = set_type_default() do mod.widgets.MapViewBase{
        width: Fill
        height: Fill
        center_lon: 4.9041
        center_lat: 52.3676
        zoom: 14.0
        min_zoom: 11.0
        max_zoom: 17.0
        use_network: false
        use_local_mbtiles: true

        draw_bg +: {
            color: #xddd7cc
        }
        draw_label +: {
            color: #x000000
            text_style: theme.font_regular{font_size: 7}
        }
        draw_text +: {
            color: #xdee9f4
            text_style: theme.font_regular{font_size: 10}
        }
    }
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawMapVector {
    #[deref]
    pub draw_super: DrawVector,
    #[live(vec2(1.0, 1.0))]
    pub map_scale: Vec2f,
    #[live(vec2(0.0, 0.0))]
    pub map_offset: Vec2f,
}

impl DrawMapVector {
    fn draw_geometry(
        &mut self,
        cx: &mut Cx2d,
        geometry_id: GeometryId,
        map_scale: Vec2f,
        map_offset: Vec2f,
    ) {
        self.map_scale = map_scale;
        self.map_offset = map_offset;

        self.draw_super.draw_vars.geometry_id = Some(geometry_id);
        cx.new_draw_call(&self.draw_super.draw_vars);
        if self.draw_super.draw_vars.can_instance() {
            let new_area = cx.add_aligned_instance(&self.draw_super.draw_vars);
            self.draw_super.draw_vars.area =
                cx.update_area_refs(self.draw_super.draw_vars.area, new_area);
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawRotatedText {
    #[deref]
    pub draw_super: DrawText,
    #[live(0.0)]
    pub rotation: f32,
    #[live(1.0)]
    pub label_scale: f32,
    #[live(vec2(0.0, 0.0))]
    pub rotation_origin: Vec2f,
}

impl DrawRotatedText {
    fn draw_rasterized_glyph_abs_transformed_anchor(
        &mut self,
        cx: &mut Cx2d,
        glyph_origin_in_lpxs: crate::makepad_draw::text::geom::Point<f32>,
        rotation_origin_in_lpxs: crate::makepad_draw::text::geom::Point<f32>,
        font_size_in_lpxs: f32,
        rasterized_glyph: crate::makepad_draw::text::rasterizer::RasterizedGlyph,
        rotation: f32,
        label_scale: f32,
    ) {
        self.rotation = rotation;
        self.label_scale = label_scale;
        self.rotation_origin = vec2(rotation_origin_in_lpxs.x, rotation_origin_in_lpxs.y);
        self.draw_super.draw_rasterized_glyph_abs(
            cx,
            glyph_origin_in_lpxs,
            font_size_in_lpxs,
            rasterized_glyph,
            self.draw_super.color,
        );
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct TileKey {
    z: u32,
    x: i32,
    y: i32,
}

#[derive(Debug)]
enum TileLoadState {
    LoadingNetwork,
    LoadingLocal,
    Ready {
        fill_geometry: Option<Geometry>,
        stroke_geometry: Option<Geometry>,
        feature_count: usize,
        labels: Vec<TileLabel>,
    },
    Failed {
        retry_after: u64,
    },
}

#[derive(Debug)]
struct TileEntry {
    state: TileLoadState,
    last_used: u64,
    attempts: u8,
}

#[derive(Debug)]
struct PendingTileRequest {
    tile_key: TileKey,
    endpoint: &'static str,
}

#[derive(Debug)]
enum LocalSourceMessage {
    Generated {
        requested: Vec<TileKey>,
        loaded: Vec<LoadedLocalTile>,
    },
    Failed {
        requested: Vec<TileKey>,
        error: String,
    },
}

#[derive(Debug)]
struct WayData {
    nodes: Vec<i64>,
    tags: HashMap<String, String>,
    closed: bool,
}

#[derive(Debug)]
struct TileBuffers {
    fill_indices: Vec<u32>,
    fill_vertices: Vec<f32>,
    stroke_indices: Vec<u32>,
    stroke_vertices: Vec<f32>,
    feature_count: usize,
    labels: Vec<TileLabel>,
}

#[derive(Clone, Debug)]
struct TileLabel {
    text: String,
    priority: u8,
    source_layer: String,
    road_kind: String,
    path_points: Vec<(f32, f32)>,
}

#[derive(Clone, Debug)]
struct LabelCandidate {
    text: String,
    name_key: String,
    road_kind: String,
    source_rank: u8,
    score: f64,
    center: Vec2d,
    repeat_distance: f64,
    font_scale: f32,
    screen_path: Vec<Vec2d>,
}

#[derive(Clone, Copy, Debug)]
struct LabelGlyphInstance {
    glyph_origin: crate::makepad_draw::text::geom::Point<f32>,
    rotation_origin: crate::makepad_draw::text::geom::Point<f32>,
    font_size_in_lpxs: f32,
    rasterized: crate::makepad_draw::text::rasterizer::RasterizedGlyph,
    angle: f32,
}

#[derive(Clone, Debug)]
struct LabelDrawPlan {
    score: f64,
    center: Vec2d,
    bounds: Rect,
    glyphs: Vec<LabelGlyphInstance>,
}

#[derive(Debug)]
struct LoadedLocalTile {
    tile_key: TileKey,
    buffers: TileBuffers,
}

#[derive(DeJson)]
struct OverpassResponse {
    elements: Vec<OverpassElement>,
}

#[derive(DeJson)]
struct OverpassElement {
    #[rename(type)]
    kind: String,
    id: i64,
    lat: Option<f64>,
    lon: Option<f64>,
    nodes: Option<Vec<i64>>,
    tags: Option<HashMap<String, String>>,
}

#[derive(Script, Widget)]
pub struct MapView {
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,

    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[redraw]
    #[live]
    draw_map: DrawMapVector,
    #[redraw]
    #[live]
    draw_label: DrawRotatedText,
    #[redraw]
    #[live]
    draw_text: DrawText,

    #[live(4.9041)]
    center_lon: f64,
    #[live(52.3676)]
    center_lat: f64,
    #[live(14.0)]
    zoom: f64,
    #[live(11.0)]
    min_zoom: f64,
    #[live(17.0)]
    max_zoom: f64,
    #[live(true)]
    use_network: bool,
    #[live(true)]
    use_local_mbtiles: bool,

    #[rust]
    center_norm: Vec2d,
    #[rust]
    view_rect: Rect,
    #[rust]
    drag_start_abs: Option<Vec2d>,
    #[rust]
    drag_start_center_norm: Vec2d,
    #[rust]
    tiles: HashMap<TileKey, TileEntry>,
    #[rust]
    request_to_tile: HashMap<LiveId, PendingTileRequest>,
    #[rust]
    next_request_id: u64,
    #[rust]
    visible_tiles: Vec<TileKey>,
    #[rust]
    frame_counter: u64,
    #[rust]
    status: String,
    #[rust]
    local_source_missing_logged: bool,
    #[rust]
    local_to_ui: ToUIReceiver<LocalSourceMessage>,
    #[rust]
    local_job_in_progress: bool,
    #[rust]
    local_requested_tiles: HashSet<TileKey>,
    #[rust]
    local_missing_tiles: HashSet<TileKey>,
}

impl ScriptHook for MapView {
    fn on_after_apply(
        &mut self,
        _vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if apply.is_eval() {
            return;
        }

        let min_zoom = self.min_zoom.max(0.0);
        let max_zoom = self.max_zoom.max(min_zoom);
        self.zoom = self.zoom.clamp(min_zoom, max_zoom);
        self.center_norm = lon_lat_to_normalized(self.center_lon, self.center_lat);
        self.wrap_and_clamp_center();
        self.normalize_source_mode();

        if self.next_request_id == 0 {
            self.next_request_id = 1;
        }
        self.ensure_cache_dir();
        if self.status.is_empty() {
            self.status = "Loading Amsterdam tiles from local cache/mbtiles...".to_string();
        }
    }
}

impl Widget for MapView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.handle_local_source_messages(cx);
        self.widget_match_event(cx, event, scope);

        match event.hits_with_capture_overload(cx, self.draw_bg.area(), true) {
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                self.drag_start_abs = Some(fe.abs);
                self.drag_start_center_norm = self.center_norm;
                cx.set_cursor(MouseCursor::Grabbing);
            }
            Hit::FingerMove(fe) => {
                if let Some(start_abs) = self.drag_start_abs {
                    let delta = fe.abs - start_abs;
                    let world_size = tile_world_size_zoom(self.view_zoom());
                    self.center_norm = self.drag_start_center_norm
                        - dvec2(delta.x / world_size, delta.y / world_size);
                    self.wrap_and_clamp_center();
                    self.redraw(cx);
                }
            }
            Hit::FingerUp(_) => {
                self.drag_start_abs = None;
                cx.set_cursor(MouseCursor::Grab);
            }
            Hit::FingerHoverIn(_) => {
                cx.set_cursor(MouseCursor::Grab);
            }
            Hit::FingerScroll(fs) => {
                let scroll = if fs.scroll.y.abs() > f64::EPSILON {
                    fs.scroll.y
                } else {
                    fs.scroll.x
                };
                self.zoom_with_anchor(cx, scroll, fs.abs);
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        self.view_rect = rect;

        self.draw_bg.draw_abs(cx, rect);
        self.ensure_visible_tiles(cx, rect);

        let view_zoom = self.view_zoom();
        let world_size = tile_world_size_zoom(view_zoom);
        let center_world = self.center_norm * world_size;
        let map_offset = Vec2f {
            x: (rect.pos.x + rect.size.x * 0.5 - center_world.x) as f32,
            y: (rect.pos.y + rect.size.y * 0.5 - center_world.y) as f32,
        };

        let mut draw_tiles = self.draw_tile_keys_with_fallback();
        draw_tiles.sort_unstable_by_key(|key| (key.z, key.y, key.x));

        for key in &draw_tiles {
            let Some(entry) = self.tiles.get(key) else {
                continue;
            };
            if let TileLoadState::Ready { fill_geometry, .. } = &entry.state {
                let Some(fill_geometry) = fill_geometry else {
                    continue;
                };
                let scale = 2.0_f64.powf(view_zoom - key.z as f64) as f32;
                self.draw_map.draw_geometry(
                    cx,
                    fill_geometry.geometry_id(),
                    Vec2f { x: scale, y: scale },
                    map_offset,
                );
            }
        }

        for key in &draw_tiles {
            let Some(entry) = self.tiles.get(key) else {
                continue;
            };
            if let TileLoadState::Ready {
                stroke_geometry, ..
            } = &entry.state
            {
                let Some(stroke_geometry) = stroke_geometry else {
                    continue;
                };
                let scale = 2.0_f64.powf(view_zoom - key.z as f64) as f32;
                self.draw_map.draw_geometry(
                    cx,
                    stroke_geometry.geometry_id(),
                    Vec2f { x: scale, y: scale },
                    map_offset,
                );
            }
        }

        if view_zoom >= 13.0 {
            self.place_and_draw_labels(cx, &draw_tiles, view_zoom, map_offset, rect);
        }

        self.draw_text.draw_abs(
            cx,
            dvec2(rect.pos.x + 10.0, rect.pos.y + 16.0),
            &self.status,
        );

        DrawStep::done()
    }
}

impl WidgetMatchEvent for MapView {
    fn handle_http_response(
        &mut self,
        cx: &mut Cx,
        request_id: LiveId,
        response: &HttpResponse,
        _scope: &mut Scope,
    ) {
        let Some(pending) = self.request_to_tile.remove(&request_id) else {
            return;
        };
        let tile_key = pending.tile_key;
        let endpoint = pending.endpoint;

        if response.status_code != 200 {
            let preview = response
                .get_string_body()
                .unwrap_or_default()
                .chars()
                .take(120)
                .collect::<String>();
            self.mark_tile_failed(
                tile_key,
                &format!(
                    "endpoint {} http status {} body: {}",
                    endpoint, response.status_code, preview
                ),
            );
            self.update_status_text();
            self.redraw(cx);
            return;
        }

        let Some(body) = response.get_string_body() else {
            self.mark_tile_failed(
                tile_key,
                &format!("endpoint {} missing utf8 response body", endpoint),
            );
            self.update_status_text();
            self.redraw(cx);
            return;
        };

        match self.build_tile_buffers(tile_key, &body) {
            Ok(buffers) => {
                self.store_tile_data_cache(tile_key, &body);
                self.insert_ready_tile(cx, tile_key, buffers);
            }
            Err(err) => {
                self.mark_tile_failed(tile_key, &format!("endpoint {} parse: {}", endpoint, err));
            }
        }

        self.update_status_text();
        self.redraw(cx);
    }

    fn handle_http_request_error(
        &mut self,
        cx: &mut Cx,
        request_id: LiveId,
        err: &HttpError,
        _scope: &mut Scope,
    ) {
        let Some(pending) = self.request_to_tile.remove(&request_id) else {
            return;
        };
        let tile_key = pending.tile_key;
        let endpoint = pending.endpoint;

        self.mark_tile_failed(
            tile_key,
            &format!("endpoint {} http request error: {:?}", endpoint, err),
        );
        self.update_status_text();
        self.redraw(cx);
    }
}

impl MapView {
    fn normalize_source_mode(&mut self) {
        if self.use_local_mbtiles && self.use_network {
            log!(
                "MapView: both sources enabled; selecting OFFLINE mode (mbtiles only). Set use_local_mbtiles:false for ONLINE mode."
            );
            self.use_network = false;
        } else if !self.use_local_mbtiles && !self.use_network {
            log!("MapView: no source enabled; selecting OFFLINE mode (mbtiles only).");
            self.use_local_mbtiles = true;
        }
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.draw_bg.redraw(cx);
    }

    fn ensure_cache_dir(&self) {
        ensure_cache_dir();
    }

    fn insert_ready_tile(&mut self, cx: &mut Cx, tile_key: TileKey, buffers: TileBuffers) {
        let TileBuffers {
            fill_indices,
            fill_vertices,
            stroke_indices,
            stroke_vertices,
            feature_count,
            labels,
        } = buffers;

        let fill_geometry = if !fill_indices.is_empty() && !fill_vertices.is_empty() {
            let geometry = Geometry::new(cx);
            geometry.update(cx, fill_indices, fill_vertices);
            Some(geometry)
        } else {
            None
        };

        let stroke_geometry = if !stroke_indices.is_empty() && !stroke_vertices.is_empty() {
            let geometry = Geometry::new(cx);
            geometry.update(cx, stroke_indices, stroke_vertices);
            Some(geometry)
        } else {
            None
        };

        self.tiles.insert(
            tile_key,
            TileEntry {
                state: TileLoadState::Ready {
                    fill_geometry,
                    stroke_geometry,
                    feature_count,
                    labels,
                },
                last_used: self.frame_counter,
                attempts: 0,
            },
        );
    }

    fn tile_data_cache_path(&self, tile_key: TileKey) -> PathBuf {
        tile_data_cache_path_for(tile_key)
    }

    fn load_tile_data_cache(&mut self, tile_key: TileKey) -> Option<String> {
        let path = self.tile_data_cache_path(tile_key);
        fs::read_to_string(path).ok()
    }

    fn store_tile_data_cache(&mut self, tile_key: TileKey, body: &str) {
        store_tile_data_cache_on_disk(tile_key, body);
    }

    fn handle_local_source_messages(&mut self, cx: &mut Cx) {
        let mut redraw = false;
        while let Ok(msg) = self.local_to_ui.try_recv() {
            self.local_job_in_progress = false;
            match msg {
                LocalSourceMessage::Generated { requested, loaded } => {
                    for key in &requested {
                        self.local_requested_tiles.remove(key);
                    }

                    if !requested.is_empty() {
                        log!(
                            "MapView: local mbtiles batch requested:{} loaded:{}",
                            requested.len(),
                            loaded.len()
                        );
                    }

                    let mut loaded_keys = HashSet::with_capacity(loaded.len());
                    let mut empty_feature_tiles = Vec::<TileKey>::new();
                    for tile in loaded {
                        let tile_key = tile.tile_key;
                        let buffers = tile.buffers;
                        loaded_keys.insert(tile_key);
                        self.local_missing_tiles.remove(&tile_key);
                        if buffers.feature_count == 0 {
                            empty_feature_tiles.push(tile_key);
                        }
                        self.insert_ready_tile(cx, tile_key, buffers)
                    }

                    if !empty_feature_tiles.is_empty() {
                        empty_feature_tiles.sort_unstable();
                        log!(
                            "MapView: local mbtiles loaded {} tile(s) with 0 rendered features sample:{}",
                            empty_feature_tiles.len(),
                            format_tile_key_sample(&empty_feature_tiles, 8)
                        );
                    }

                    for key in requested {
                        if loaded_keys.contains(&key) {
                            continue;
                        }
                        self.local_missing_tiles.insert(key);
                        if self.use_network {
                            self.tiles.remove(&key);
                        } else {
                            self.tiles.remove(&key);
                        }
                    }
                    redraw = true;
                }
                LocalSourceMessage::Failed { requested, error } => {
                    log!("MapView: local mbtiles load failed: {}", error);
                    for key in requested {
                        self.local_requested_tiles.remove(&key);
                        if self.use_network {
                            self.tiles.remove(&key);
                        } else {
                            self.tiles.remove(&key);
                        }
                    }
                    redraw = true;
                }
            }
        }

        if redraw {
            self.update_status_text();
            self.redraw(cx);
        }
    }

    fn request_visible_tiles_from_local_source(&mut self, cx: &mut Cx) {
        if !self.use_local_mbtiles || self.local_job_in_progress {
            return;
        }

        let mbtiles_path = Path::new(LOCAL_MBTILES_PATH);
        if !mbtiles_path.is_file() {
            if !self.local_source_missing_logged {
                log!(
                    "MapView: local mbtiles source missing at {} (set use_local_mbtiles: false to disable)",
                    LOCAL_MBTILES_PATH
                );
                self.local_source_missing_logged = true;
            }
            return;
        }

        let mut missing = Vec::<TileKey>::new();
        for key in &self.visible_tiles {
            if self.tiles.contains_key(key)
                || self.local_requested_tiles.contains(key)
                || self.local_missing_tiles.contains(key)
            {
                continue;
            }
            missing.push(*key);
        }

        if missing.is_empty() {
            return;
        }

        if missing.len() > MAX_LOCAL_TILE_BATCH {
            missing.truncate(MAX_LOCAL_TILE_BATCH);
        }

        for key in &missing {
            self.local_requested_tiles.insert(*key);
            self.tiles.insert(
                *key,
                TileEntry {
                    state: TileLoadState::LoadingLocal,
                    last_used: self.frame_counter,
                    attempts: 0,
                },
            );
        }

        self.local_job_in_progress = true;
        let sender = self.local_to_ui.sender();
        let requested = missing.clone();
        let mbtiles_path = LOCAL_MBTILES_PATH.to_string();
        let cache_dir = TILE_CACHE_DIR.to_string();

        cx.spawn_thread(move || {
            let result =
                load_local_tile_batch(Path::new(&mbtiles_path), Path::new(&cache_dir), &requested);
            match result {
                Ok(loaded) => {
                    let _ = sender.send(LocalSourceMessage::Generated { requested, loaded });
                }
                Err(error) => {
                    let _ = sender.send(LocalSourceMessage::Failed { requested, error });
                }
            }
        });
    }

    fn mark_tile_failed(&mut self, tile_key: TileKey, reason: &str) {
        let attempts = self
            .tiles
            .get(&tile_key)
            .map_or(1, |entry| entry.attempts.saturating_add(1));
        let retry_delay = retry_delay_frames(attempts);
        let retry_after = self.frame_counter.saturating_add(retry_delay);

        self.tiles.insert(
            tile_key,
            TileEntry {
                state: TileLoadState::Failed { retry_after },
                last_used: self.frame_counter,
                attempts,
            },
        );

        log!(
            "MapView: tile z{} x{} y{} failed (attempt {}): {}",
            tile_key.z,
            tile_key.x,
            tile_key.y,
            attempts,
            reason
        );
    }

    fn wrap_and_clamp_center(&mut self) {
        self.center_norm.x = self.center_norm.x.rem_euclid(1.0);
        self.center_norm.y = self.center_norm.y.clamp(0.0, 1.0);
    }

    fn zoom_with_anchor(&mut self, cx: &mut Cx, scroll: f64, anchor_abs: Vec2d) {
        if scroll.abs() <= f64::EPSILON {
            return;
        }

        let current_zoom = self.view_zoom();
        let zoom_delta = (-scroll / 240.0).clamp(-1.0, 1.0);
        let min_zoom = self.min_zoom.max(0.0);
        let max_zoom = self.max_zoom.max(min_zoom);
        let new_zoom = (current_zoom + zoom_delta).clamp(min_zoom, max_zoom);
        if (new_zoom - current_zoom).abs() < 1e-4 {
            return;
        }

        if self.view_rect.size.x <= 0.0 || self.view_rect.size.y <= 0.0 {
            self.zoom = new_zoom;
            self.redraw(cx);
            return;
        }

        let old_world_size = tile_world_size_zoom(current_zoom);
        let new_world_size = tile_world_size_zoom(new_zoom);

        let rect_center = self.view_rect.pos + self.view_rect.size * 0.5;
        let old_center_world = self.center_norm * old_world_size;
        let anchor_world = old_center_world + (anchor_abs - rect_center);
        let anchor_norm = anchor_world / old_world_size;
        let new_center_world = anchor_norm * new_world_size - (anchor_abs - rect_center);

        self.zoom = new_zoom;
        self.center_norm = new_center_world / new_world_size;
        self.wrap_and_clamp_center();
        self.redraw(cx);
    }

    fn ensure_visible_tiles(&mut self, cx: &mut Cx, rect: Rect) {
        self.frame_counter = self.frame_counter.wrapping_add(1);
        self.visible_tiles = self.visible_tile_keys(rect);
        let target_zoom = self.request_zoom_level();

        self.request_visible_tiles_from_local_source(cx);

        let mut visible_set = HashSet::with_capacity(self.visible_tiles.len());
        for key in &self.visible_tiles {
            visible_set.insert(*key);
            if let Some(entry) = self.tiles.get_mut(key) {
                entry.last_used = self.frame_counter;
            }
        }

        let mut pending = self
            .tiles
            .values()
            .filter(|entry| matches!(entry.state, TileLoadState::LoadingNetwork))
            .count();

        for key in self.visible_tiles.clone() {
            let retry_attempt = self.tiles.get(&key).and_then(|entry| {
                if let TileLoadState::Failed { retry_after } = entry.state {
                    if entry.attempts < MAX_TILE_RETRIES && self.frame_counter >= retry_after {
                        return Some(entry.attempts);
                    }
                }
                None
            });
            if let Some(attempts) = retry_attempt {
                if pending < MAX_PENDING_REQUESTS && self.request_tile(cx, key, attempts, true) {
                    pending += 1;
                }
                continue;
            }

            if self.tiles.contains_key(&key) {
                continue;
            }

            if self.local_missing_tiles.contains(&key) {
                if self.use_network
                    && pending < MAX_PENDING_REQUESTS
                    && self.request_tile(cx, key, 0, true)
                {
                    pending += 1;
                }
                continue;
            }

            if self.request_tile(cx, key, 0, pending < MAX_PENDING_REQUESTS) {
                pending += 1;
            }
        }

        if self.tiles.len() > 640 {
            let frame_counter = self.frame_counter;
            let min_keep_zoom = target_zoom.saturating_sub(2);
            let max_keep_zoom = target_zoom.saturating_add(1);
            self.tiles.retain(|key, entry| {
                if visible_set.contains(key)
                    || matches!(
                        entry.state,
                        TileLoadState::LoadingNetwork | TileLoadState::LoadingLocal
                    )
                {
                    return true;
                }
                if key.z < min_keep_zoom || key.z > max_keep_zoom {
                    return false;
                }
                frame_counter.saturating_sub(entry.last_used) <= 240
            });
        }

        self.update_status_text();
    }

    fn visible_tile_keys(&self, rect: Rect) -> Vec<TileKey> {
        if rect.size.x <= 0.0 || rect.size.y <= 0.0 {
            return Vec::new();
        }

        let zoom = self.request_zoom_level();
        let world_size = tile_world_size(zoom);
        let center_world = self.center_norm * world_size;
        let half_size = dvec2(rect.size.x * 0.5, rect.size.y * 0.5);
        let top_left = center_world - half_size;
        let bottom_right = center_world + half_size;
        let tile_count = 1_i32 << zoom;

        let min_tx = (top_left.x / TILE_SIZE).floor() as i32 - 1;
        let max_tx = (bottom_right.x / TILE_SIZE).ceil() as i32 + 1;
        let min_ty = (top_left.y / TILE_SIZE).floor() as i32 - 1;
        let max_ty = (bottom_right.y / TILE_SIZE).ceil() as i32 + 1;

        let mut out = Vec::new();
        for ty in min_ty..=max_ty {
            if ty < 0 || ty >= tile_count {
                continue;
            }
            for tx in min_tx..=max_tx {
                out.push(TileKey {
                    z: zoom,
                    x: tx.rem_euclid(tile_count),
                    y: ty,
                });
            }
        }

        out.sort_unstable();
        out.dedup();

        let center_tx = (center_world.x / TILE_SIZE).floor() as i32;
        let center_ty = (center_world.y / TILE_SIZE).floor() as i32;
        out.sort_unstable_by_key(|key| {
            let dx = (key.x - center_tx).abs();
            let dy = (key.y - center_ty).abs();
            (dx + dy, key.y, key.x)
        });

        out
    }

    fn draw_tile_keys_with_fallback(&self) -> Vec<TileKey> {
        let mut out = Vec::with_capacity(self.visible_tiles.len());
        let mut seen = HashSet::with_capacity(self.visible_tiles.len() * 2);

        for key in &self.visible_tiles {
            if self.tile_is_ready(*key) {
                if seen.insert(*key) {
                    out.push(*key);
                }
                continue;
            }

            if let Some(draw_key) = self.find_ready_ancestor(*key) {
                if seen.insert(draw_key) {
                    out.push(draw_key);
                }
                continue;
            }

            for draw_key in self.find_ready_descendants(*key) {
                if seen.insert(draw_key) {
                    out.push(draw_key);
                }
            }
        }

        out
    }

    fn tile_is_ready(&self, key: TileKey) -> bool {
        self.tiles.get(&key).is_some_and(|entry| {
            if let TileLoadState::Ready {
                fill_geometry,
                stroke_geometry,
                feature_count,
                ..
            } = &entry.state
            {
                *feature_count > 0 || fill_geometry.is_some() || stroke_geometry.is_some()
            } else {
                false
            }
        })
    }

    fn find_ready_ancestor(&self, mut key: TileKey) -> Option<TileKey> {
        while key.z > 0 {
            key = TileKey {
                z: key.z - 1,
                x: key.x / 2,
                y: key.y / 2,
            };
            if self.tile_is_ready(key) {
                return Some(key);
            }
        }
        None
    }

    fn find_ready_descendants(&self, key: TileKey) -> Vec<TileKey> {
        let mut out = Vec::new();
        for (candidate, entry) in &self.tiles {
            if !matches!(entry.state, TileLoadState::Ready { .. }) {
                continue;
            }
            if is_descendant_tile(*candidate, key) {
                out.push(*candidate);
            }
        }
        out
    }

    fn request_tile(
        &mut self,
        cx: &mut Cx,
        tile_key: TileKey,
        attempts: u8,
        allow_network: bool,
    ) -> bool {
        if attempts == 0 && !self.use_local_mbtiles {
            if let Some(cached_body) = self.load_tile_data_cache(tile_key) {
                match self.build_tile_buffers(tile_key, &cached_body) {
                    Ok(buffers) => {
                        self.insert_ready_tile(cx, tile_key, buffers);
                        return false;
                    }
                    Err(err) => {
                        log!(
                            "MapView: cache parse failed for tile z{} x{} y{}: {}",
                            tile_key.z,
                            tile_key.x,
                            tile_key.y,
                            err
                        );
                        let _ = fs::remove_file(self.tile_data_cache_path(tile_key));
                    }
                }
            }
        }

        if !allow_network || !self.use_network {
            return false;
        }

        let request_id = LiveId(self.next_request_id);
        self.next_request_id = self.next_request_id.wrapping_add(1);
        if self.next_request_id == 0 {
            self.next_request_id = 1;
        }

        let query = overpass_query(tile_key);
        let endpoint = overpass_endpoint(attempts);
        let mut request = HttpRequest::new(endpoint.to_string(), HttpMethod::POST);
        request.set_header("Content-Type".to_string(), "text/plain".to_string());
        request.set_header("Accept".to_string(), "application/json".to_string());
        request.set_header("User-Agent".to_string(), "makepad-map-view".to_string());
        request.set_body_string(&query);

        self.request_to_tile
            .insert(request_id, PendingTileRequest { tile_key, endpoint });
        self.tiles.insert(
            tile_key,
            TileEntry {
                state: TileLoadState::LoadingNetwork,
                last_used: self.frame_counter,
                attempts,
            },
        );
        cx.http_request(request_id, request);
        true
    }

    fn build_tile_buffers(&self, tile_key: TileKey, body: &str) -> Result<TileBuffers, String> {
        build_tile_buffers_from_body(tile_key, body)
    }

    fn place_and_draw_labels(
        &mut self,
        cx: &mut Cx2d,
        draw_tiles: &[TileKey],
        view_zoom: f64,
        map_offset: Vec2f,
        rect: Rect,
    ) {
        let mut candidates = self.collect_label_candidates(draw_tiles, view_zoom, map_offset, rect);
        if candidates.is_empty() {
            return;
        }
        candidates.sort_unstable_by(|a, b| b.score.total_cmp(&a.score));

        let mut accepted_centers = HashMap::<String, Vec<Vec2d>>::new();
        let mut accepted_bounds = Vec::<Rect>::new();
        let mut plans = Vec::<LabelDrawPlan>::new();

        for candidate in candidates {
            let repeat_key = format!("{}|{}", candidate.name_key, candidate.road_kind);
            let close_repeat = accepted_centers
                .get(&repeat_key)
                .is_some_and(|centers| {
                    let r2 = candidate.repeat_distance * candidate.repeat_distance;
                    centers.iter().any(|center| {
                        let dx = center.x - candidate.center.x;
                        let dy = center.y - candidate.center.y;
                        dx * dx + dy * dy < r2
                    })
                });
            if close_repeat {
                continue;
            }

            let Some(plan) = self.build_label_draw_plan(cx, &candidate) else {
                continue;
            };
            if rect_outside_rect(plan.bounds, rect, LABEL_VIEW_MARGIN) {
                continue;
            }
            if accepted_bounds.iter().any(|placed| {
                rects_overlap_with_padding(*placed, plan.bounds, LABEL_COLLISION_PADDING)
            }) {
                continue;
            }

            accepted_centers.entry(repeat_key).or_default().push(plan.center);
            accepted_bounds.push(plan.bounds);
            plans.push(plan);
        }

        plans.sort_unstable_by(|a, b| a.score.total_cmp(&b.score));
        for plan in &plans {
            self.draw_label_plan(cx, plan);
        }
    }

    fn collect_label_candidates(
        &self,
        draw_tiles: &[TileKey],
        view_zoom: f64,
        map_offset: Vec2f,
        rect: Rect,
    ) -> Vec<LabelCandidate> {
        let mut out = Vec::<LabelCandidate>::new();

        for key in draw_tiles {
            let (labels, scale) = {
                let Some(entry) = self.tiles.get(key) else {
                    continue;
                };
                let TileLoadState::Ready { labels, .. } = &entry.state else {
                    continue;
                };
                if labels.is_empty() {
                    continue;
                }
                (labels.clone(), 2.0_f64.powf(view_zoom - key.z as f64) as f32)
            };

            let zoom_delta = (view_zoom - key.z as f64).abs();
            for label in labels {
                let Some(source_rank) = label_source_rank(&label.source_layer) else {
                    continue;
                };
                let name_key = normalize_label_key(label.text.as_str());
                if name_key.len() < 2 {
                    continue;
                }

                let screen_path = build_screen_polyline(&label.path_points, scale, map_offset);
                if screen_path.len() < 2 || polyline_outside_rect(&screen_path, rect, LABEL_VIEW_MARGIN)
                {
                    continue;
                }
                let cumulative = polyline_cumulative_lengths(&screen_path);
                let path_length = *cumulative.last().unwrap_or(&0.0);
                if path_length < LABEL_MIN_PATH_PIXELS {
                    continue;
                }
                let Some(center) = sample_polyline_point_at_distance(
                    &screen_path,
                    &cumulative,
                    path_length * 0.5,
                ) else {
                    continue;
                };

                let repeat_distance = repeat_distance_for_label(label.priority, source_rank);
                let mut font_scale = (scale.powf(0.28) * 0.78).clamp(0.52, 1.05);
                font_scale *= match label.priority {
                    1 => 1.08,
                    2 => 1.0,
                    _ => 0.92,
                };

                let score = source_rank as f64 * 1000.0
                    + (4_u8.saturating_sub(label.priority) as f64) * 120.0
                    + (220.0 - zoom_delta * 65.0)
                    + path_length.min(640.0) * 0.35;

                out.push(LabelCandidate {
                    text: label.text,
                    name_key,
                    road_kind: label.road_kind,
                    source_rank,
                    score,
                    center,
                    repeat_distance,
                    font_scale,
                    screen_path,
                });
            }
        }

        out
    }

    fn build_label_draw_plan(
        &mut self,
        cx: &mut Cx2d,
        candidate: &LabelCandidate,
    ) -> Option<LabelDrawPlan> {
        if candidate.screen_path.len() < 2 {
            return None;
        }

        self.draw_label.draw_super.font_scale = candidate.font_scale;
        let run = self
            .draw_label
            .draw_super
            .prepare_single_line_run(cx, candidate.text.as_str())?;
        if run.glyphs.is_empty() {
            return None;
        }

        let cumulative = polyline_cumulative_lengths(&candidate.screen_path);
        let total_length = *cumulative.last()?;
        let text_width = run.width_in_lpxs;
        if total_length < text_width as f64 + 4.0 {
            return None;
        }

        let baseline_shift = (run.ascender_in_lpxs + run.descender_in_lpxs) * 0.5;
        let start_distance = (total_length - text_width as f64) * 0.5;
        let probe_delta = (text_width as f64 * 0.25).clamp(12.0, 42.0);
        let mid_distance = start_distance + text_width as f64 * 0.5;
        let reverse = sample_polyline_tangent_angle_raw(
            &candidate.screen_path,
            &cumulative,
            mid_distance,
            probe_delta,
        )
        .map(|angle| angle.cos() < 0.0)
        .unwrap_or(false);

        let label_half_height = ((run.ascender_in_lpxs - run.descender_in_lpxs).abs() as f64 * 0.5)
            .max(3.0);
        let mut min_x = f64::INFINITY;
        let mut min_y = f64::INFINITY;
        let mut max_x = f64::NEG_INFINITY;
        let mut max_y = f64::NEG_INFINITY;
        let mut glyphs = Vec::<LabelGlyphInstance>::with_capacity(run.glyphs.len());
        let mut prev_angle: Option<f32> = None;

        for glyph in &run.glyphs {
            if glyph.advance_in_lpxs <= 0.0 {
                continue;
            }

            let glyph_center_distance =
                start_distance + (glyph.pen_x_in_lpxs + glyph.advance_in_lpxs * 0.5) as f64;
            let path_center_distance = if reverse {
                total_length - glyph_center_distance
            } else {
                glyph_center_distance
            };

            let Some(center_point) = sample_polyline_point_at_distance(
                &candidate.screen_path,
                &cumulative,
                path_center_distance,
            ) else {
                continue;
            };

            let angle_sample_delta = (glyph.advance_in_lpxs as f64 * 1.25).clamp(6.0, 24.0);
            let Some(raw_angle) = sample_polyline_tangent_angle_raw(
                &candidate.screen_path,
                &cumulative,
                path_center_distance,
                angle_sample_delta,
            ) else {
                continue;
            };
            let mut angle = upright_angle(raw_angle);
            if let Some(prev) = prev_angle {
                angle = smooth_continuous_angle(prev, angle, 0.65);
            }
            prev_angle = Some(angle);

            let tangent = dvec2((angle as f64).cos(), (angle as f64).sin());
            let normal = dvec2(-tangent.y, tangent.x);
            let baseline_center = center_point + normal * baseline_shift as f64;
            let baseline_pen_origin =
                baseline_center - tangent * (glyph.advance_in_lpxs as f64 * 0.5);
            let glyph_origin = baseline_pen_origin + tangent * glyph.offset_x_in_lpxs as f64;

            let half_width = (glyph.advance_in_lpxs.abs() as f64 * 0.62).max(2.0);
            min_x = min_x.min(baseline_center.x - half_width);
            min_y = min_y.min(baseline_center.y - label_half_height);
            max_x = max_x.max(baseline_center.x + half_width);
            max_y = max_y.max(baseline_center.y + label_half_height);

            glyphs.push(LabelGlyphInstance {
                glyph_origin: crate::makepad_draw::text::geom::Point::new(
                    glyph_origin.x as f32,
                    glyph_origin.y as f32,
                ),
                rotation_origin: crate::makepad_draw::text::geom::Point::new(
                    baseline_center.x as f32,
                    baseline_center.y as f32,
                ),
                font_size_in_lpxs: glyph.font_size_in_lpxs,
                rasterized: glyph.rasterized,
                angle,
            });
        }

        if glyphs.is_empty() || !min_x.is_finite() || !min_y.is_finite() {
            return None;
        }

        let bounds = rect(
            min_x - 2.0,
            min_y - 2.0,
            (max_x - min_x + 4.0).max(1.0),
            (max_y - min_y + 4.0).max(1.0),
        );

        Some(LabelDrawPlan {
            score: candidate.score + candidate.source_rank as f64 * 2.0,
            center: candidate.center,
            bounds,
            glyphs,
        })
    }

    fn draw_label_plan(&mut self, cx: &mut Cx2d, plan: &LabelDrawPlan) {
        for glyph in &plan.glyphs {
            self.draw_label.draw_rasterized_glyph_abs_transformed_anchor(
                cx,
                glyph.glyph_origin,
                glyph.rotation_origin,
                glyph.font_size_in_lpxs,
                glyph.rasterized,
                glyph.angle,
                1.0,
            );
        }
    }

    fn update_status_text(&mut self) {
        let mut ready = 0usize;
        let mut loading = 0usize;
        let mut failed = 0usize;
        let mut retrying = 0usize;
        let mut exhausted = 0usize;
        let mut features = 0usize;

        for key in &self.visible_tiles {
            let Some(entry) = self.tiles.get(key) else {
                continue;
            };
            match &entry.state {
                TileLoadState::LoadingNetwork | TileLoadState::LoadingLocal => loading += 1,
                TileLoadState::Ready { feature_count, .. } => {
                    ready += 1;
                    features += *feature_count;
                }
                TileLoadState::Failed { .. } => {
                    failed += 1;
                    if entry.attempts >= MAX_TILE_RETRIES {
                        exhausted += 1;
                    } else {
                        retrying += 1;
                    }
                }
            }
        }

        self.status = format!(
            "Amsterdam [{}] z{:.2} (req:{})  ready:{}  loading:{}  failed:{}(retry:{} stuck:{})  features:{}",
            self.source_mode_label(),
            self.view_zoom(),
            self.request_zoom_level(),
            ready,
            loading,
            failed,
            retrying,
            exhausted,
            features
        );
    }

    fn view_zoom(&self) -> f64 {
        let min = self.min_zoom.max(0.0);
        let max = self.max_zoom.max(min);
        self.zoom.clamp(min, max)
    }

    fn request_zoom_level(&self) -> u32 {
        let mut zoom = self.view_zoom().round() as u32;
        if self.use_local_mbtiles {
            zoom = zoom.clamp(LOCAL_MBTILES_MIN_ZOOM, LOCAL_MBTILES_MAX_ZOOM);
        }
        zoom
    }

    fn source_mode_label(&self) -> &'static str {
        if self.use_local_mbtiles {
            "offline"
        } else if self.use_network {
            "online"
        } else {
            "disabled"
        }
    }
}

fn retry_delay_frames(attempts: u8) -> u64 {
    let shift = attempts.saturating_sub(1).min(6) as u32;
    let delay = RETRY_BASE_FRAMES.saturating_mul(1_u64 << shift);
    delay.min(RETRY_MAX_FRAMES)
}

fn is_descendant_tile(child: TileKey, parent: TileKey) -> bool {
    if child.z <= parent.z {
        return false;
    }

    let dz = child.z - parent.z;
    if dz >= 31 {
        return false;
    }

    let min_x = (parent.x as i64) << dz;
    let max_x = ((parent.x as i64 + 1) << dz) - 1;
    let min_y = (parent.y as i64) << dz;
    let max_y = ((parent.y as i64 + 1) << dz) - 1;
    let cx = child.x as i64;
    let cy = child.y as i64;

    cx >= min_x && cx <= max_x && cy >= min_y && cy <= max_y
}

fn emit_path(path: &mut VectorPath, points: &[(f32, f32)], close: bool) {
    if points.len() < 2 {
        return;
    }
    path.move_to(points[0].0, points[0].1);
    for point in points.iter().skip(1) {
        path.line_to(point.0, point.1);
    }
    if close {
        path.close();
    }
}

fn hex_to_premul_rgba(hex: u32, alpha: f32) -> [f32; 4] {
    let r = ((hex >> 16) & 0xff) as f32 / 255.0;
    let g = ((hex >> 8) & 0xff) as f32 / 255.0;
    let b = (hex & 0xff) as f32 / 255.0;
    [r * alpha, g * alpha, b * alpha, alpha]
}

fn simplify_label_path(points: &[(f32, f32)]) -> Vec<(f32, f32)> {
    if points.len() <= 256 {
        return points.to_vec();
    }
    let step = (points.len() / 256).max(1);
    let mut out = Vec::with_capacity(258);
    for (index, point) in points.iter().enumerate() {
        if index == 0 || index + 1 == points.len() || index % step == 0 {
            out.push(*point);
        }
    }
    out
}

fn build_screen_polyline(path_points: &[(f32, f32)], scale: f32, map_offset: Vec2f) -> Vec<Vec2d> {
    let mut out = Vec::<Vec2d>::with_capacity(path_points.len());
    for &(x, y) in path_points {
        out.push(dvec2(
            x as f64 * scale as f64 + map_offset.x as f64,
            y as f64 * scale as f64 + map_offset.y as f64,
        ));
    }
    out
}

fn polyline_outside_rect(points: &[Vec2d], rect: Rect, margin: f64) -> bool {
    if points.is_empty() {
        return true;
    }
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for point in points {
        min_x = min_x.min(point.x);
        min_y = min_y.min(point.y);
        max_x = max_x.max(point.x);
        max_y = max_y.max(point.y);
    }
    max_x < rect.pos.x - margin
        || max_y < rect.pos.y - margin
        || min_x > rect.pos.x + rect.size.x + margin
        || min_y > rect.pos.y + rect.size.y + margin
}

fn polyline_cumulative_lengths(points: &[Vec2d]) -> Vec<f64> {
    let mut out = Vec::with_capacity(points.len());
    let mut sum = 0.0_f64;
    out.push(sum);
    for pair in points.windows(2) {
        let dx = pair[1].x - pair[0].x;
        let dy = pair[1].y - pair[0].y;
        sum += (dx * dx + dy * dy).sqrt();
        out.push(sum);
    }
    out
}

fn wrap_angle_pi(mut angle: f32) -> f32 {
    while angle > std::f32::consts::PI {
        angle -= std::f32::consts::TAU;
    }
    while angle < -std::f32::consts::PI {
        angle += std::f32::consts::TAU;
    }
    angle
}

fn upright_angle(raw_angle: f32) -> f32 {
    let mut angle = wrap_angle_pi(raw_angle);
    if angle.cos() < 0.0 {
        angle = wrap_angle_pi(angle + std::f32::consts::PI);
    }
    angle
}

fn smooth_continuous_angle(previous: f32, current: f32, blend: f32) -> f32 {
    let mut next = current;
    while next - previous > std::f32::consts::PI {
        next -= std::f32::consts::TAU;
    }
    while next - previous < -std::f32::consts::PI {
        next += std::f32::consts::TAU;
    }
    let blend = blend.clamp(0.0, 1.0);
    previous + (next - previous) * blend
}

fn normalize_label_key(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev_space = true;
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_space = false;
        } else if ch.is_whitespace() && !prev_space {
            out.push(' ');
            prev_space = true;
        }
    }
    out.trim().to_string()
}

fn label_source_rank(layer: &str) -> Option<u8> {
    if layer.is_empty() {
        return Some(4);
    }
    Some(match layer {
        "street_labels" | "street_labels_points" => 7,
        "transportation_name" => 6,
        "transportation" | "road" | "streets" | "bridges"
        | "aerialways" | "ferries" | "public_transport" => 2,
        _ => return None,
    })
}

fn repeat_distance_for_label(priority: u8, source_rank: u8) -> f64 {
    let base = match priority {
        1 => 220.0,
        2 => 170.0,
        _ => 120.0,
    };
    base + (source_rank as f64 - 4.0) * 10.0
}

fn rects_overlap_with_padding(a: Rect, b: Rect, padding: f64) -> bool {
    let ax0 = a.pos.x - padding;
    let ay0 = a.pos.y - padding;
    let ax1 = a.pos.x + a.size.x + padding;
    let ay1 = a.pos.y + a.size.y + padding;
    let bx0 = b.pos.x - padding;
    let by0 = b.pos.y - padding;
    let bx1 = b.pos.x + b.size.x + padding;
    let by1 = b.pos.y + b.size.y + padding;
    ax0 < bx1 && ax1 > bx0 && ay0 < by1 && ay1 > by0
}

fn rect_outside_rect(a: Rect, b: Rect, margin: f64) -> bool {
    a.pos.x + a.size.x < b.pos.x - margin
        || a.pos.y + a.size.y < b.pos.y - margin
        || a.pos.x > b.pos.x + b.size.x + margin
        || a.pos.y > b.pos.y + b.size.y + margin
}

fn select_label_text(tags: &HashMap<String, String>) -> Option<String> {
    for key in ["name", "name:latin", "name:en", "name_int"] {
        if let Some(value) = tags.get(key) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    if let Some(reference) = tags.get("ref") {
        let trimmed = reference.trim();
        if !trimmed.is_empty() && trimmed.len() <= 12 {
            return Some(trimmed.to_string());
        }
    }
    None
}

fn is_road_polygon_layer(layer: &str) -> bool {
    matches!(layer, "street_polygons" | "streets_polygons_labels")
}

fn sample_polyline_point_at_distance(
    points: &[Vec2d],
    cumulative: &[f64],
    distance: f64,
) -> Option<Vec2d> {
    if points.len() < 2 || cumulative.len() != points.len() {
        return None;
    }

    let total = *cumulative.last()?;
    let clamped = distance.clamp(0.0, total);
    for i in 0..points.len() - 1 {
        let start = cumulative[i];
        let end = cumulative[i + 1];
        if clamped > end && i + 2 < points.len() {
            continue;
        }
        let seg_len = (end - start).max(1e-6);
        let t = ((clamped - start) / seg_len).clamp(0.0, 1.0);
        let a = points[i];
        let b = points[i + 1];
        let pos = dvec2(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t);
        return Some(pos);
    }
    None
}

fn sample_polyline_tangent_angle_raw(
    points: &[Vec2d],
    cumulative: &[f64],
    distance: f64,
    delta: f64,
) -> Option<f32> {
    let total = *cumulative.last()?;
    if total <= 1e-6 {
        return None;
    }
    let d0 = (distance - delta).max(0.0);
    let d1 = (distance + delta).min(total);
    let p0 = sample_polyline_point_at_distance(points, cumulative, d0)?;
    let p1 = sample_polyline_point_at_distance(points, cumulative, d1)?;
    let dx = p1.x - p0.x;
    let dy = p1.y - p0.y;
    if dx.abs() < 1e-6 && dy.abs() < 1e-6 {
        return None;
    }
    Some(dy.atan2(dx) as f32)
}

fn extract_way_label(tags: &HashMap<String, String>, points: &[(f32, f32)]) -> Option<TileLabel> {
    if points.len() < 2 {
        return None;
    }
    if !tags.contains_key("highway") {
        return None;
    }
    let source_layer = tags.get("layer").cloned().unwrap_or_default();
    if is_road_polygon_layer(&source_layer) {
        return None;
    }
    if label_source_rank(&source_layer).is_none() {
        return None;
    }
    let name = select_label_text(tags)?;

    let road_kind = tags
        .get("highway")
        .cloned()
        .unwrap_or_else(|| "residential".to_string());
    let priority = match road_kind.as_str() {
        "motorway" | "trunk" | "primary" => 1,
        "secondary" | "tertiary" => 2,
        _ => 3,
    };

    let path_points = simplify_label_path(points);
    if path_points.len() < 2 {
        return None;
    }
    Some(TileLabel {
        text: name,
        priority,
        source_layer,
        road_kind,
        path_points,
    })
}

fn build_tile_buffers_from_body(tile_key: TileKey, body: &str) -> Result<TileBuffers, String> {
    let parsed = OverpassResponse::deserialize_json_lenient(body)
        .map_err(|e| format!("json error at line {} col {}: {}", e.line, e.col, e.msg))?;

    let mut nodes = HashMap::<i64, (f64, f64)>::new();
    let mut ways = Vec::<WayData>::new();

    for element in parsed.elements {
        match element.kind.as_str() {
            "node" => {
                if let (Some(lat), Some(lon)) = (element.lat, element.lon) {
                    nodes.insert(element.id, (lon, lat));
                }
            }
            "way" => {
                if let Some(node_ids) = element.nodes {
                    let closed =
                        node_ids.len() > 2 && node_ids.first().copied() == node_ids.last().copied();
                    ways.push(WayData {
                        nodes: node_ids,
                        tags: element.tags.unwrap_or_default(),
                        closed,
                    });
                }
            }
            _ => {}
        }
    }

    let mut path = VectorPath::new();
    let mut tess = Tessellator::default();
    let mut tess_verts = Vec::<VVertex>::new();
    let mut tess_indices = Vec::<u32>::new();

    let mut fill_indices = Vec::<u32>::new();
    let mut fill_vertices = Vec::<f32>::new();
    let mut stroke_indices = Vec::<u32>::new();
    let mut stroke_vertices = Vec::<f32>::new();
    let mut fill_zbias = 0.0_f32;
    let mut stroke_zbias = 0.0_f32;
    let mut feature_count = 0usize;
    let mut labels = Vec::<TileLabel>::new();

    for way in &ways {
        let points = project_way_points(&way.nodes, &nodes, tile_key.z);
        if points.len() < 2 {
            continue;
        }

        if points.len() >= 3 {
            if let Some(color) = fill_color(&way.tags, way.closed) {
                emit_path(&mut path, &points, true);
                tessellate_path_fill(
                    &mut path,
                    &mut tess,
                    &mut tess_verts,
                    &mut tess_indices,
                    LineJoin::Miter,
                    4.0,
                    1.0,
                    false,
                );
                append_tessellated_geometry(
                    &tess_verts,
                    &tess_indices,
                    &mut fill_vertices,
                    &mut fill_indices,
                    VectorRenderParams {
                        color: hex_to_premul_rgba(color, 1.0),
                        stroke_mult: 1e6,
                        shape_id: 0.0,
                        params: [0.0; 6],
                        zbias: fill_zbias,
                    },
                );
                fill_zbias += VECTOR_ZBIAS_STEP;
                feature_count += 1;
            }
        }

        if let Some((color, width)) = stroke_style(&way.tags) {
            emit_path(&mut path, &points, false);
            let stroke_mult = tessellate_path_stroke(
                &mut path,
                &mut tess,
                &mut tess_verts,
                &mut tess_indices,
                width,
                LineCap::Round,
                LineJoin::Round,
                4.0,
                1.0,
            );
            append_tessellated_geometry(
                &tess_verts,
                &tess_indices,
                &mut stroke_vertices,
                &mut stroke_indices,
                VectorRenderParams {
                    color: hex_to_premul_rgba(color, 1.0),
                    stroke_mult,
                    shape_id: 0.0,
                    params: [0.0; 6],
                    zbias: stroke_zbias,
                },
            );
            stroke_zbias += VECTOR_ZBIAS_STEP;
            feature_count += 1;

            if let Some(label) = extract_way_label(&way.tags, &points) {
                labels.push(label);
            }
        }
    }

    labels.sort_unstable_by_key(|label| label.priority);
    if labels.len() > 96 {
        labels.truncate(96);
    }

    Ok(TileBuffers {
        fill_indices,
        fill_vertices,
        stroke_indices,
        stroke_vertices,
        feature_count,
        labels,
    })
}

fn project_way_points(
    node_ids: &[i64],
    nodes: &HashMap<i64, (f64, f64)>,
    zoom: u32,
) -> Vec<(f32, f32)> {
    let mut out = Vec::with_capacity(node_ids.len());
    let mut last: Option<(f32, f32)> = None;

    for node_id in node_ids {
        let Some((lon, lat)) = nodes.get(node_id).copied() else {
            continue;
        };
        let world = lon_lat_to_world(lon, lat, zoom);
        let point = (world.x as f32, world.y as f32);

        if let Some(prev) = last {
            let dx = point.0 - prev.0;
            let dy = point.1 - prev.1;
            if dx * dx + dy * dy < 0.25 {
                continue;
            }
        }

        out.push(point);
        last = Some(point);
    }

    out
}

fn fill_color(tags: &HashMap<String, String>, closed: bool) -> Option<u32> {
    if !closed {
        return None;
    }

    if tags.contains_key("building") {
        return Some(0xc6c0b5);
    }

    if tag_is(tags, "natural", "water") || tag_is(tags, "waterway", "riverbank") {
        return Some(0x9ecff2);
    }

    if let Some(landuse) = tags.get("landuse") {
        return Some(match landuse.as_str() {
            "residential" => 0xe9e4dc,
            "commercial" | "retail" => 0xe1dbd2,
            "industrial" => 0xd6d1cb,
            "forest" => 0xc4deb0,
            "grass" | "meadow" | "farmland" => 0xd4e5bf,
            _ => 0xe5dfd6,
        });
    }

    if let Some(leisure) = tags.get("leisure") {
        return Some(match leisure.as_str() {
            "park" | "garden" | "golf_course" => 0xc5e2b6,
            "pitch" => 0xb8db9f,
            _ => 0xd1e8bf,
        });
    }

    None
}

fn stroke_style(tags: &HashMap<String, String>) -> Option<(u32, f32)> {
    let layer = tags.get("layer").map(|value| value.as_str()).unwrap_or("");
    if is_road_polygon_layer(layer) {
        return None;
    }

    if let Some(highway) = tags.get("highway") {
        return Some(match highway.as_str() {
            "motorway" => (0xe3ae67, 2.8),
            "trunk" => (0xe9c07c, 2.5),
            "primary" => (0xf1d39b, 2.2),
            "secondary" => (0xf3e0ba, 1.8),
            "tertiary" => (0xf4e8cf, 1.45),
            "residential" | "service" | "unclassified" | "living_street" => (0xf5f4f1, 1.0),
            "cycleway" | "footway" | "path" | "pedestrian" => (0xc7c0b3, 0.62),
            _ => (0xe8e3db, 0.92),
        });
    }

    if let Some(waterway) = tags.get("waterway") {
        let width = match waterway.as_str() {
            "river" => 1.35,
            "canal" => 1.05,
            _ => 0.78,
        };
        return Some((0x5b9fd4, width));
    }

    if tags.contains_key("railway") {
        return Some((0x928e86, 0.72));
    }

    None
}

fn tag_is(tags: &HashMap<String, String>, key: &str, value: &str) -> bool {
    tags.get(key).is_some_and(|v| v == value)
}

fn lon_lat_to_normalized(lon: f64, lat: f64) -> Vec2d {
    let x = (lon + 180.0) / 360.0;
    let clamped_lat = lat.clamp(-85.051_128_78, 85.051_128_78);
    let sin_lat = clamped_lat.to_radians().sin();
    let y = 0.5 - ((1.0 + sin_lat) / (1.0 - sin_lat)).ln() / (4.0 * std::f64::consts::PI);
    dvec2(x, y)
}

fn lon_lat_to_world(lon: f64, lat: f64, zoom: u32) -> Vec2d {
    lon_lat_to_normalized(lon, lat) * tile_world_size(zoom)
}

fn tile_world_size(zoom: u32) -> f64 {
    tile_world_size_zoom(zoom as f64)
}

fn tile_world_size_zoom(zoom: f64) -> f64 {
    TILE_SIZE * 2.0_f64.powf(zoom)
}

fn tile_corner_lon_lat_f64(x: f64, y: f64, zoom: u32) -> (f64, f64) {
    let n = 2.0_f64.powi(zoom as i32);
    let lon = x / n * 360.0 - 180.0;
    let lat_rad = (std::f64::consts::PI * (1.0 - 2.0 * y / n)).sinh().atan();
    let lat = lat_rad.to_degrees();
    (lon, lat)
}

fn tile_bounds_padded(tile: TileKey, pad_tiles: f64) -> (f64, f64, f64, f64) {
    let (west, north) =
        tile_corner_lon_lat_f64(tile.x as f64 - pad_tiles, tile.y as f64 - pad_tiles, tile.z);
    let (east, south) = tile_corner_lon_lat_f64(
        tile.x as f64 + 1.0 + pad_tiles,
        tile.y as f64 + 1.0 + pad_tiles,
        tile.z,
    );
    (south, west, north, east)
}

fn overpass_endpoint(attempts: u8) -> &'static str {
    let index = attempts as usize % OVERPASS_ENDPOINTS.len();
    OVERPASS_ENDPOINTS[index]
}

fn overpass_query(tile: TileKey) -> String {
    let (south, west, north, east) = tile_bounds_padded(tile, TILE_QUERY_PAD);
    let mut ways = String::new();

    ways.push_str(&format!(
        "way[\"highway\"]({south:.6},{west:.6},{north:.6},{east:.6});\
         way[\"waterway\"]({south:.6},{west:.6},{north:.6},{east:.6});\
         way[\"natural\"=\"water\"]({south:.6},{west:.6},{north:.6},{east:.6});"
    ));

    // Buildings are by far the biggest payload at z14 in dense cities like Amsterdam.
    if tile.z >= 15 {
        ways.push_str(&format!(
            "way[\"building\"][\"building\"!=\"no\"]({south:.6},{west:.6},{north:.6},{east:.6});"
        ));
    }

    if tile.z >= 14 {
        ways.push_str(&format!(
            "way[\"landuse\"]({south:.6},{west:.6},{north:.6},{east:.6});\
             way[\"leisure\"]({south:.6},{west:.6},{north:.6},{east:.6});"
        ));
    }

    format!(
        "[out:json][timeout:20];\
         ({ways});\
         (._;>;);\
         out body;"
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MvtGeomType {
    Unknown,
    Point,
    LineString,
    Polygon,
}

impl MvtGeomType {
    fn from_u64(value: u64) -> Self {
        match value {
            1 => Self::Point,
            2 => Self::LineString,
            3 => Self::Polygon,
            _ => Self::Unknown,
        }
    }
}

#[derive(Clone, Debug)]
enum MvtValue {
    String(String),
    Float(f32),
    Double(f64),
    Int(i64),
    UInt(u64),
    SInt(i64),
    Bool(bool),
}

impl MvtValue {
    fn to_tag_string(&self) -> String {
        match self {
            Self::String(value) => value.clone(),
            Self::Float(value) => format!("{}", value),
            Self::Double(value) => format!("{}", value),
            Self::Int(value) => format!("{}", value),
            Self::UInt(value) => format!("{}", value),
            Self::SInt(value) => format!("{}", value),
            Self::Bool(value) => {
                if *value {
                    "true".to_string()
                } else {
                    "false".to_string()
                }
            }
        }
    }
}

#[derive(Debug)]
struct MvtTileJsonBuilder {
    nodes: Vec<(i64, f64, f64)>,
    node_map: HashMap<(i32, i32), i64>,
    ways: Vec<(i64, Vec<i64>, HashMap<String, String>)>,
    next_node_id: i64,
    next_way_id: i64,
}

impl Default for MvtTileJsonBuilder {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            node_map: HashMap::new(),
            ways: Vec::new(),
            next_node_id: 1,
            next_way_id: 1,
        }
    }
}

impl MvtTileJsonBuilder {
    fn add_path(
        &mut self,
        tile_key: TileKey,
        extent: u32,
        points: &[(i32, i32)],
        tags: HashMap<String, String>,
        close: bool,
    ) {
        if points.len() < 2 {
            return;
        }

        let mut node_ids = Vec::with_capacity(points.len() + 1);
        for &(x, y) in points {
            let node_id = if let Some(existing) = self.node_map.get(&(x, y)).copied() {
                existing
            } else {
                let id = self.next_node_id;
                self.next_node_id += 1;
                self.node_map.insert((x, y), id);
                let (lon, lat) = local_tile_to_lon_lat(tile_key, extent, x, y);
                self.nodes.push((id, lon, lat));
                id
            };

            if node_ids.last().copied() != Some(node_id) {
                node_ids.push(node_id);
            }
        }

        if node_ids.len() < 2 {
            return;
        }

        if close && node_ids.first().copied() != node_ids.last().copied() {
            if let Some(first) = node_ids.first().copied() {
                node_ids.push(first);
            }
        }

        if node_ids.len() < 2 {
            return;
        }

        let way_id = self.next_way_id;
        self.next_way_id += 1;
        self.ways.push((way_id, node_ids, tags));
    }

    fn to_json(&self) -> String {
        let mut out = String::with_capacity(32 + self.nodes.len() * 64 + self.ways.len() * 192);
        out.push_str("{\"elements\":[");
        let mut first = true;

        for &(id, lon, lat) in &self.nodes {
            if !first {
                out.push(',');
            }
            first = false;
            out.push_str("{\"type\":\"node\",\"id\":");
            out.push_str(&id.to_string());
            out.push_str(",\"lat\":");
            out.push_str(&format!("{:.8}", lat));
            out.push_str(",\"lon\":");
            out.push_str(&format!("{:.8}", lon));
            out.push('}');
        }

        for (id, node_ids, tags) in &self.ways {
            if !first {
                out.push(',');
            }
            first = false;
            out.push_str("{\"type\":\"way\",\"id\":");
            out.push_str(&id.to_string());
            out.push_str(",\"nodes\":[");
            for (index, node_id) in node_ids.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&node_id.to_string());
            }
            out.push_str("],\"tags\":{");
            let mut tag_first = true;
            for (key, value) in tags {
                if !tag_first {
                    out.push(',');
                }
                tag_first = false;
                append_json_string(&mut out, key);
                out.push(':');
                append_json_string(&mut out, value);
            }
            out.push_str("}}");
        }

        out.push_str("]}");
        out
    }
}

fn format_tile_key_sample(keys: &[TileKey], limit: usize) -> String {
    if keys.is_empty() {
        return "[]".to_string();
    }
    let mut out = String::from("[");
    for (index, key) in keys.iter().take(limit).enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        out.push_str(&format!("z{}x{}y{}", key.z, key.x, key.y));
    }
    if keys.len() > limit {
        out.push_str(", ...");
    }
    out.push(']');
    out
}

fn ensure_cache_dir() {
    let _ = fs::create_dir_all(TILE_CACHE_DIR);
}

fn tile_data_cache_path_for(tile_key: TileKey) -> PathBuf {
    Path::new(TILE_CACHE_DIR).join(format!(
        "z{}_x{}_y{}.json",
        tile_key.z, tile_key.x, tile_key.y
    ))
}

fn store_tile_data_cache_on_disk(tile_key: TileKey, body: &str) {
    ensure_cache_dir();
    let path = tile_data_cache_path_for(tile_key);
    let tmp = path.with_extension("tmp");
    if fs::write(&tmp, body).is_err() {
        let _ = fs::remove_file(&tmp);
        return;
    }
    let _ = fs::rename(&tmp, &path);
}

fn load_local_tile_batch(
    mbtiles_path: &Path,
    cache_dir: &Path,
    requested: &[TileKey],
) -> Result<Vec<LoadedLocalTile>, String> {
    if requested.is_empty() {
        return Ok(Vec::new());
    }

    let mut loaded = Vec::<LoadedLocalTile>::new();
    let mut missing = Vec::<TileKey>::new();
    for key in requested {
        let cache_path = cache_dir.join(format!("z{}_x{}_y{}.json", key.z, key.x, key.y));
        match fs::read_to_string(&cache_path) {
            Ok(body) => match build_tile_buffers_from_body(*key, &body) {
                Ok(buffers) => loaded.push(LoadedLocalTile {
                    tile_key: *key,
                    buffers,
                }),
                Err(err) => {
                    log!(
                        "MapView: cache parse failed for tile z{} x{} y{}: {}",
                        key.z,
                        key.x,
                        key.y,
                        err
                    );
                    let _ = fs::remove_file(cache_path);
                    missing.push(*key);
                }
            },
            Err(_) => missing.push(*key),
        }
    }

    if missing.is_empty() {
        return Ok(loaded);
    }

    let mut reader = MbtilesReader::open(mbtiles_path)
        .map_err(|err| format!("open {}: {}", mbtiles_path.display(), err))?;

    let mut by_zoom = HashMap::<u32, Vec<TileKey>>::new();
    for key in &missing {
        by_zoom.entry(key.z).or_default().push(*key);
    }

    let mut logged_xyz_row_scheme = false;

    for (zoom, keys) in by_zoom {
        let tile_count = 1_i64 << zoom;
        let mut needed_tms = HashMap::<(i64, i64), TileKey>::new();
        let mut needed_xyz = HashMap::<(i64, i64), TileKey>::new();
        for key in keys {
            let x = key.x as i64;
            let xyz_row = key.y as i64;
            let tms_row = tile_count - 1 - key.y as i64;
            needed_tms.insert((x, tms_row), key);
            needed_xyz.insert((x, xyz_row), key);
        }

        let tiles = reader.get_tiles_at_zoom(zoom as i64).map_err(|err| {
            format!(
                "read zoom {} from {}: {}",
                zoom,
                mbtiles_path.display(),
                err
            )
        })?;

        for tile in tiles {
            let lookup = (tile.tile_column, tile.tile_row);

            let matched = if let Some(tile_key) = needed_tms.remove(&lookup) {
                let xyz_lookup = (tile_key.x as i64, tile_key.y as i64);
                needed_xyz.remove(&xyz_lookup);
                Some((tile_key, false))
            } else if let Some(tile_key) = needed_xyz.remove(&lookup) {
                let tms_lookup = (tile_key.x as i64, tile_count - 1 - tile_key.y as i64);
                needed_tms.remove(&tms_lookup);
                Some((tile_key, true))
            } else {
                None
            };

            let Some((tile_key, used_xyz_row)) = matched else {
                continue;
            };

            if used_xyz_row && !logged_xyz_row_scheme {
                log!(
                    "MapView: local mbtiles rows appear XYZ-oriented (matched without TMS row flip)"
                );
                logged_xyz_row_scheme = true;
            }

            match mbtiles_tile_to_overpass_json(tile_key, &tile.tile_data) {
                Ok(body) => match build_tile_buffers_from_body(tile_key, &body) {
                    Ok(buffers) => {
                        store_tile_data_cache_on_disk(tile_key, &body);
                        loaded.push(LoadedLocalTile { tile_key, buffers });
                    }
                    Err(err) => {
                        log!(
                            "MapView: failed to triangulate local mbtile z{} x{} y{}: {}",
                            tile_key.z,
                            tile_key.x,
                            tile_key.y,
                            err
                        );
                    }
                },
                Err(err) => {
                    log!(
                        "MapView: failed to decode local mbtile z{} x{} y{}: {}",
                        tile_key.z,
                        tile_key.x,
                        tile_key.y,
                        err
                    );
                }
            }
        }

        if !needed_tms.is_empty() {
            let mut missing = needed_tms.values().copied().collect::<Vec<_>>();
            missing.sort_unstable();
            log!(
                "MapView: local mbtiles missing {} tile(s) at z{} sample:{}",
                missing.len(),
                zoom,
                format_tile_key_sample(&missing, 8)
            );
        }
    }

    Ok(loaded)
}

fn mbtiles_tile_to_overpass_json(
    tile_key: TileKey,
    raw_tile_data: &[u8],
) -> Result<String, String> {
    let pbf_data = decode_vector_tile_payload(raw_tile_data)?;
    let mut builder = MvtTileJsonBuilder::default();
    parse_mvt_tile(&pbf_data, tile_key, &mut builder)?;
    Ok(builder.to_json())
}

fn decode_vector_tile_payload(raw: &[u8]) -> Result<Vec<u8>, String> {
    if raw.len() >= 2 && raw[0] == 0x1f && raw[1] == 0x8b {
        let mut decoder = GzDecoder::new(raw);
        let mut out = Vec::new();
        decoder
            .read_to_end(&mut out)
            .map_err(|err| format!("gzip decode failed: {}", err))?;
        return Ok(out);
    }

    if raw.len() >= 2 && raw[0] == 0x78 {
        let mut decoder = ZlibDecoder::new(raw);
        let mut out = Vec::new();
        if decoder.read_to_end(&mut out).is_ok() {
            return Ok(out);
        }
    }

    Ok(raw.to_vec())
}

fn parse_mvt_tile(
    tile_data: &[u8],
    tile_key: TileKey,
    builder: &mut MvtTileJsonBuilder,
) -> Result<(), String> {
    let mut pos = 0_usize;
    while pos < tile_data.len() {
        let key = read_pb_varint(tile_data, &mut pos)?;
        let field = (key >> 3) as u32;
        let wire = (key & 0x7) as u8;
        match (field, wire) {
            (3, 2) => {
                let layer = read_pb_len_slice(tile_data, &mut pos)?;
                parse_mvt_layer(layer, tile_key, builder)?;
            }
            _ => skip_pb_field(tile_data, &mut pos, wire)?,
        }
    }
    Ok(())
}

fn parse_mvt_layer(
    layer_data: &[u8],
    tile_key: TileKey,
    builder: &mut MvtTileJsonBuilder,
) -> Result<(), String> {
    let mut pos = 0_usize;
    let mut layer_name = String::new();
    let mut extent = 4096_u32;
    let mut features = Vec::<&[u8]>::new();
    let mut keys = Vec::<String>::new();
    let mut values = Vec::<MvtValue>::new();

    while pos < layer_data.len() {
        let key = read_pb_varint(layer_data, &mut pos)?;
        let field = (key >> 3) as u32;
        let wire = (key & 0x7) as u8;
        match (field, wire) {
            (1, 2) => {
                let slice = read_pb_len_slice(layer_data, &mut pos)?;
                layer_name = String::from_utf8_lossy(slice).into_owned();
            }
            (2, 2) => features.push(read_pb_len_slice(layer_data, &mut pos)?),
            (3, 2) => {
                let slice = read_pb_len_slice(layer_data, &mut pos)?;
                keys.push(String::from_utf8_lossy(slice).into_owned());
            }
            (4, 2) => {
                let value = parse_mvt_value(read_pb_len_slice(layer_data, &mut pos)?)?;
                values.push(value);
            }
            (5, 0) => extent = read_pb_varint(layer_data, &mut pos)? as u32,
            _ => skip_pb_field(layer_data, &mut pos, wire)?,
        }
    }

    let extent = extent.max(1);
    for feature_data in features {
        parse_mvt_feature(
            feature_data,
            &layer_name,
            &keys,
            &values,
            extent,
            tile_key,
            builder,
        )?;
    }

    Ok(())
}

fn parse_mvt_feature(
    feature_data: &[u8],
    layer_name: &str,
    keys: &[String],
    values: &[MvtValue],
    extent: u32,
    tile_key: TileKey,
    builder: &mut MvtTileJsonBuilder,
) -> Result<(), String> {
    let mut pos = 0_usize;
    let mut tag_indexes = Vec::<u32>::new();
    let mut geom_type = MvtGeomType::Unknown;
    let mut geometry_cmds = Vec::<u32>::new();

    while pos < feature_data.len() {
        let key = read_pb_varint(feature_data, &mut pos)?;
        let field = (key >> 3) as u32;
        let wire = (key & 0x7) as u8;
        match (field, wire) {
            (2, 2) => {
                let packed = read_pb_len_slice(feature_data, &mut pos)?;
                tag_indexes = read_packed_u32(packed)?;
            }
            (3, 0) => geom_type = MvtGeomType::from_u64(read_pb_varint(feature_data, &mut pos)?),
            (4, 2) => {
                let packed = read_pb_len_slice(feature_data, &mut pos)?;
                geometry_cmds = read_packed_u32(packed)?;
            }
            _ => skip_pb_field(feature_data, &mut pos, wire)?,
        }
    }

    if matches!(geom_type, MvtGeomType::Unknown | MvtGeomType::Point) {
        return Ok(());
    }

    let mut tags = HashMap::<String, String>::new();
    for pair in tag_indexes.chunks_exact(2) {
        let key_index = pair[0] as usize;
        let value_index = pair[1] as usize;
        let Some(key) = keys.get(key_index) else {
            continue;
        };
        let Some(value) = values.get(value_index) else {
            continue;
        };
        tags.insert(key.clone(), value.to_tag_string());
    }
    normalize_mvt_tags(layer_name, geom_type, &mut tags);

    let paths = decode_mvt_geometry(&geometry_cmds, geom_type)?;
    for mut path in paths {
        if path.len() < 2 {
            continue;
        }
        let close = geom_type == MvtGeomType::Polygon;
        if close && path.first().copied() != path.last().copied() {
            if let Some(first) = path.first().copied() {
                path.push(first);
            }
        }
        if close && path.len() < 4 {
            continue;
        }
        builder.add_path(tile_key, extent, &path, tags.clone(), close);
    }

    Ok(())
}

fn normalize_mvt_tags(
    layer_name: &str,
    geom_type: MvtGeomType,
    tags: &mut HashMap<String, String>,
) {
    tags.entry("layer".to_string())
        .or_insert_with(|| layer_name.to_string());

    match layer_name {
        "building" | "buildings" => {
            tags.entry("building".to_string())
                .or_insert_with(|| "yes".to_string());
        }
        "water" | "water_polygons" | "water_polygons_labels" | "ocean" => {
            if geom_type == MvtGeomType::Polygon {
                tags.entry("natural".to_string())
                    .or_insert_with(|| "water".to_string());
            } else {
                tags.entry("waterway".to_string())
                    .or_insert_with(|| "river".to_string());
            }
        }
        "waterway" | "water_lines" | "water_lines_labels" | "dam_lines" | "pier_lines" => {
            let value = tags
                .get("kind")
                .cloned()
                .or_else(|| tags.get("subclass").cloned())
                .or_else(|| tags.get("class").cloned())
                .unwrap_or_else(|| "river".to_string());
            tags.entry("waterway".to_string()).or_insert(value);
        }
        "transportation"
        | "transportation_name"
        | "road"
        | "streets"
        | "street_polygons"
        | "street_labels"
        | "street_labels_points"
        | "streets_polygons_labels"
        | "bridges"
        | "aerialways"
        | "ferries"
        | "public_transport" => {
            let value = tags
                .get("kind")
                .cloned()
                .or_else(|| tags.get("subclass").cloned())
                .or_else(|| tags.get("class").cloned())
                .unwrap_or_else(|| "residential".to_string());
            tags.entry("highway".to_string())
                .or_insert_with(|| normalize_highway_kind(&value));
        }
        "railway" => {
            tags.entry("railway".to_string())
                .or_insert_with(|| "rail".to_string());
        }
        "park" => {
            tags.entry("leisure".to_string())
                .or_insert_with(|| "park".to_string());
        }
        "landuse" | "landcover" | "land" | "sites" | "pois" => {
            let value = tags
                .get("kind")
                .cloned()
                .or_else(|| tags.get("class").cloned())
                .or_else(|| tags.get("subclass").cloned())
                .unwrap_or_else(|| "residential".to_string());
            if is_leisure_kind(&value) {
                tags.entry("leisure".to_string())
                    .or_insert_with(|| "park".to_string());
            } else {
                tags.entry("landuse".to_string()).or_insert(value);
            }
        }
        _ => {}
    }
}

fn normalize_highway_kind(kind: &str) -> String {
    match kind {
        "motorway_link" => "motorway".to_string(),
        "trunk_link" => "trunk".to_string(),
        "primary_link" => "primary".to_string(),
        "secondary_link" => "secondary".to_string(),
        "tertiary_link" => "tertiary".to_string(),
        "major_road" => "primary".to_string(),
        "minor_road" => "residential".to_string(),
        "path" => "path".to_string(),
        other => other.to_string(),
    }
}

fn is_leisure_kind(kind: &str) -> bool {
    matches!(
        kind,
        "park" | "garden" | "playground" | "golf_course" | "pitch" | "sports_centre"
    )
}

fn parse_mvt_value(bytes: &[u8]) -> Result<MvtValue, String> {
    let mut pos = 0_usize;
    let mut value = MvtValue::String(String::new());
    while pos < bytes.len() {
        let key = read_pb_varint(bytes, &mut pos)?;
        let field = (key >> 3) as u32;
        let wire = (key & 0x7) as u8;
        match (field, wire) {
            (1, 2) => {
                let slice = read_pb_len_slice(bytes, &mut pos)?;
                value = MvtValue::String(String::from_utf8_lossy(slice).into_owned());
            }
            (2, 5) => {
                let bits = read_pb_fixed32(bytes, &mut pos)?;
                value = MvtValue::Float(f32::from_bits(bits));
            }
            (3, 1) => {
                let bits = read_pb_fixed64(bytes, &mut pos)?;
                value = MvtValue::Double(f64::from_bits(bits));
            }
            (4, 0) => value = MvtValue::Int(read_pb_varint(bytes, &mut pos)? as i64),
            (5, 0) => value = MvtValue::UInt(read_pb_varint(bytes, &mut pos)?),
            (6, 0) => value = MvtValue::SInt(zigzag_decode_u64(read_pb_varint(bytes, &mut pos)?)),
            (7, 0) => value = MvtValue::Bool(read_pb_varint(bytes, &mut pos)? != 0),
            _ => skip_pb_field(bytes, &mut pos, wire)?,
        }
    }
    Ok(value)
}

fn decode_mvt_geometry(
    commands: &[u32],
    geom_type: MvtGeomType,
) -> Result<Vec<Vec<(i32, i32)>>, String> {
    let mut parts = Vec::<Vec<(i32, i32)>>::new();
    let mut current = Vec::<(i32, i32)>::new();
    let mut x = 0_i32;
    let mut y = 0_i32;
    let mut index = 0_usize;

    while index < commands.len() {
        let header = commands[index];
        index += 1;
        let command_id = header & 0x7;
        let count = header >> 3;

        match command_id {
            1 => {
                for _ in 0..count {
                    if index + 1 >= commands.len() {
                        return Err("mvt geometry move_to missing arguments".to_string());
                    }
                    x = x.wrapping_add(zigzag_decode_u32(commands[index]));
                    y = y.wrapping_add(zigzag_decode_u32(commands[index + 1]));
                    index += 2;
                    if !current.is_empty() {
                        parts.push(current);
                        current = Vec::new();
                    }
                    current.push((x, y));
                }
            }
            2 => {
                for _ in 0..count {
                    if index + 1 >= commands.len() {
                        return Err("mvt geometry line_to missing arguments".to_string());
                    }
                    x = x.wrapping_add(zigzag_decode_u32(commands[index]));
                    y = y.wrapping_add(zigzag_decode_u32(commands[index + 1]));
                    index += 2;
                    current.push((x, y));
                }
            }
            7 => {
                if geom_type == MvtGeomType::Polygon && !current.is_empty() {
                    let first = current[0];
                    if current.last().copied() != Some(first) {
                        current.push(first);
                    }
                }
            }
            _ => return Err(format!("mvt geometry unknown command {}", command_id)),
        }
    }

    if !current.is_empty() {
        parts.push(current);
    }

    Ok(parts)
}

fn local_tile_to_lon_lat(tile_key: TileKey, extent: u32, x: i32, y: i32) -> (f64, f64) {
    let extent = extent.max(1) as f64;
    let n = 2.0_f64.powi(tile_key.z as i32);
    let tile_x = tile_key.x as f64 + x as f64 / extent;
    let tile_y = tile_key.y as f64 + y as f64 / extent;
    let lon = tile_x / n * 360.0 - 180.0;
    let lat_rad = (std::f64::consts::PI * (1.0 - 2.0 * tile_y / n))
        .sinh()
        .atan();
    (lon, lat_rad.to_degrees())
}

fn zigzag_decode_u32(value: u32) -> i32 {
    ((value >> 1) as i32) ^ (-((value & 1) as i32))
}

fn zigzag_decode_u64(value: u64) -> i64 {
    ((value >> 1) as i64) ^ (-((value & 1) as i64))
}

fn read_packed_u32(bytes: &[u8]) -> Result<Vec<u32>, String> {
    let mut pos = 0_usize;
    let mut out = Vec::new();
    while pos < bytes.len() {
        out.push(read_pb_varint(bytes, &mut pos)? as u32);
    }
    Ok(out)
}

fn read_pb_fixed32(bytes: &[u8], pos: &mut usize) -> Result<u32, String> {
    if *pos + 4 > bytes.len() {
        return Err("unexpected eof reading fixed32".to_string());
    }
    let value = u32::from_le_bytes([
        bytes[*pos],
        bytes[*pos + 1],
        bytes[*pos + 2],
        bytes[*pos + 3],
    ]);
    *pos += 4;
    Ok(value)
}

fn read_pb_fixed64(bytes: &[u8], pos: &mut usize) -> Result<u64, String> {
    if *pos + 8 > bytes.len() {
        return Err("unexpected eof reading fixed64".to_string());
    }
    let value = u64::from_le_bytes([
        bytes[*pos],
        bytes[*pos + 1],
        bytes[*pos + 2],
        bytes[*pos + 3],
        bytes[*pos + 4],
        bytes[*pos + 5],
        bytes[*pos + 6],
        bytes[*pos + 7],
    ]);
    *pos += 8;
    Ok(value)
}

fn read_pb_varint(bytes: &[u8], pos: &mut usize) -> Result<u64, String> {
    let mut value = 0_u64;
    let mut shift = 0_u32;
    while *pos < bytes.len() {
        let byte = bytes[*pos];
        *pos += 1;
        value |= ((byte & 0x7f) as u64) << shift;
        if (byte & 0x80) == 0 {
            return Ok(value);
        }
        shift += 7;
        if shift > 63 {
            return Err("varint too long".to_string());
        }
    }
    Err("unexpected eof reading varint".to_string())
}

fn read_pb_len_slice<'a>(bytes: &'a [u8], pos: &mut usize) -> Result<&'a [u8], String> {
    let len = read_pb_varint(bytes, pos)? as usize;
    if *pos + len > bytes.len() {
        return Err("unexpected eof reading length-delimited field".to_string());
    }
    let slice = &bytes[*pos..*pos + len];
    *pos += len;
    Ok(slice)
}

fn skip_pb_field(bytes: &[u8], pos: &mut usize, wire: u8) -> Result<(), String> {
    match wire {
        0 => {
            let _ = read_pb_varint(bytes, pos)?;
            Ok(())
        }
        1 => {
            if *pos + 8 > bytes.len() {
                return Err("unexpected eof skipping 64-bit field".to_string());
            }
            *pos += 8;
            Ok(())
        }
        2 => {
            let len = read_pb_varint(bytes, pos)? as usize;
            if *pos + len > bytes.len() {
                return Err("unexpected eof skipping length-delimited field".to_string());
            }
            *pos += len;
            Ok(())
        }
        5 => {
            if *pos + 4 > bytes.len() {
                return Err("unexpected eof skipping 32-bit field".to_string());
            }
            *pos += 4;
            Ok(())
        }
        _ => Err(format!("unsupported protobuf wire type {}", wire)),
    }
}

fn append_json_string(out: &mut String, text: &str) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c <= '\u{1f}' => {
                out.push_str("\\u");
                out.push_str(&format!("{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}
