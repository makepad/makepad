use super::geometry::*;
use super::label::*;
use super::style::*;
use super::tile::*;
use crate::{
    makepad_derive_widget::*, makepad_draw::*, widget::*, DrawRotatedText, DrawVector,
    PathGlyphInstance, PathTextPlacement, WidgetMatchEvent,
};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
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
            )

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

        get_stroke_mask: fn() {
            if self.v_shape_id > 9.5 && self.v_shape_id < 10.5 {
                return self.dash(3.2, 2.4)
            }
            if self.v_shape_id > 10.5 && self.v_shape_id < 11.5 {
                return self.dash(2.0, 3.0)
            }
            return 1.0
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
        dark_theme: false
        use_network: false
        use_local_mbtiles: true
        style_light: MapThemeStyle{
            background: #xddd7cc
            status_text: #xdee9f4
            label: #x000000

            MapFillRule{group: "building" color: #xc6c0b5}
            MapFillRule{group: "water" color: #x9ecff2}
            MapFillRule{group: "landuse" value: "residential" color: #xe9e4dc}
            MapFillRule{group: "landuse" value: "commercial" color: #xe1dbd2}
            MapFillRule{group: "landuse" value: "retail" color: #xe1dbd2}
            MapFillRule{group: "landuse" value: "industrial" color: #xd6d1cb}
            MapFillRule{group: "landuse" value: "forest" color: #xc4deb0}
            MapFillRule{group: "landuse" value: "grass" color: #xd4e5bf}
            MapFillRule{group: "landuse" value: "meadow" color: #xd4e5bf}
            MapFillRule{group: "landuse" value: "farmland" color: #xd4e5bf}
            MapFillRule{group: "landuse" value: "*" color: #xe5dfd6}
            MapFillRule{group: "leisure" value: "park" color: #xc5e2b6}
            MapFillRule{group: "leisure" value: "garden" color: #xc5e2b6}
            MapFillRule{group: "leisure" value: "golf_course" color: #xc5e2b6}
            MapFillRule{group: "leisure" value: "pitch" color: #xb8db9f}
            MapFillRule{group: "leisure" value: "*" color: #xd1e8bf}

            MapRoadRule{kind: "motorway" sort_rank: 700 casing_color: #xc38d49 casing_width: 3.9 center_color: #xe2ad65 center_width: 3.0}
            MapRoadRule{kind: "trunk" sort_rank: 640 casing_color: #xc59f5f casing_width: 3.5 center_color: #xe8c17e center_width: 2.7}
            MapRoadRule{kind: "primary" sort_rank: 560 casing_color: #xc6b181 casing_width: 3.1 center_color: #xf0d39c center_width: 2.35}
            MapRoadRule{kind: "secondary" sort_rank: 470 casing_color: #xd0c8b6 casing_width: 2.75 center_color: #xf4e4c4 center_width: 2.0}
            MapRoadRule{kind: "busway" sort_rank: 470 casing_color: #xd0c8b6 casing_width: 2.75 center_color: #xf4e4c4 center_width: 2.0}
            MapRoadRule{kind: "tertiary" sort_rank: 390 casing_color: #xc6c0b3 casing_width: 2.4 center_color: #xf5ebd8 center_width: 1.7}
            MapRoadRule{kind: "residential" sort_rank: 310 casing_color: #xc2bcae casing_width: 2.0 center_color: #xfefefd center_width: 1.35}
            MapRoadRule{kind: "unclassified" sort_rank: 310 casing_color: #xc2bcae casing_width: 2.0 center_color: #xfefefd center_width: 1.35}
            MapRoadRule{kind: "living_street" sort_rank: 310 casing_color: #xc2bcae casing_width: 2.0 center_color: #xfefefd center_width: 1.35}
            MapRoadRule{kind: "service" sort_rank: 240 casing_color: #xc5bfb2 casing_width: 1.75 center_color: #xf6f2ea center_width: 1.1}
            MapRoadRule{kind: "pedestrian" sort_rank: 240 casing_color: #xc5bfb2 casing_width: 1.75 center_color: #xf6f2ea center_width: 1.1}
            MapRoadRule{kind: "cycleway" sort_rank: 160 center_color: #xb6afa1 center_width: 0.82}
            MapRoadRule{kind: "footway" sort_rank: 160 center_color: #xb6afa1 center_width: 0.82}
            MapRoadRule{kind: "path" sort_rank: 160 center_color: #xb6afa1 center_width: 0.82}
            MapRoadRule{kind: "steps" sort_rank: 160 center_color: #xb6afa1 center_width: 0.82}
            MapRoadRule{kind: "track" sort_rank: 160 center_color: #xb6afa1 center_width: 0.82}
            MapRoadRule{kind: "*" sort_rank: 280 casing_color: #xc3bcaf casing_width: 1.9 center_color: #xf5f1e9 center_width: 1.2}

            MapWaterwayRule{kind: "river" sort_rank: 140 casing_color: #x4a8fc3 casing_width: 1.83 center_color: #x73b5e4 center_width: 1.55}
            MapWaterwayRule{kind: "canal" sort_rank: 140 casing_color: #x4a8fc3 casing_width: 1.5 center_color: #x73b5e4 center_width: 1.22}
            MapWaterwayRule{kind: "stream" sort_rank: 140 casing_color: #x4a8fc3 casing_width: 1.18 center_color: #x73b5e4 center_width: 0.9}
            MapWaterwayRule{kind: "*" sort_rank: 140 casing_color: #x4a8fc3 casing_width: 1.1 center_color: #x73b5e4 center_width: 0.82}
            MapRailRule{sort_rank: 180 casing_color: #xb7b2a9 casing_width: 0.96 center_color: #x8f8a81 center_width: 0.62 center_shape_id: 10.0}
        }
        style_dark: MapThemeStyle{
            background: #x161b22
            status_text: #xb2c7d8
            label: #xe5eaf1

            MapFillRule{group: "building" color: #x383d46}
            MapFillRule{group: "water" color: #x204f74}
            MapFillRule{group: "landuse" value: "residential" color: #x2a2f36}
            MapFillRule{group: "landuse" value: "commercial" color: #x30343b}
            MapFillRule{group: "landuse" value: "retail" color: #x30343b}
            MapFillRule{group: "landuse" value: "industrial" color: #x282c32}
            MapFillRule{group: "landuse" value: "forest" color: #x243629}
            MapFillRule{group: "landuse" value: "grass" color: #x2a3c2d}
            MapFillRule{group: "landuse" value: "meadow" color: #x2a3c2d}
            MapFillRule{group: "landuse" value: "farmland" color: #x2a3c2d}
            MapFillRule{group: "landuse" value: "*" color: #x2d3239}
            MapFillRule{group: "leisure" value: "park" color: #x2f4a34}
            MapFillRule{group: "leisure" value: "garden" color: #x2f4a34}
            MapFillRule{group: "leisure" value: "golf_course" color: #x2f4a34}
            MapFillRule{group: "leisure" value: "pitch" color: #x32553a}
            MapFillRule{group: "leisure" value: "*" color: #x2b4230}

            MapRoadRule{kind: "motorway" sort_rank: 700 casing_color: #x8f6937 casing_width: 3.9 center_color: #xd29b54 center_width: 3.0}
            MapRoadRule{kind: "trunk" sort_rank: 640 casing_color: #x8c7141 casing_width: 3.5 center_color: #xc8a561 center_width: 2.7}
            MapRoadRule{kind: "primary" sort_rank: 560 casing_color: #x706857 casing_width: 3.1 center_color: #xb9aa86 center_width: 2.35}
            MapRoadRule{kind: "secondary" sort_rank: 470 casing_color: #x556170 casing_width: 2.75 center_color: #x95a1b1 center_width: 2.0}
            MapRoadRule{kind: "busway" sort_rank: 470 casing_color: #x556170 casing_width: 2.75 center_color: #x95a1b1 center_width: 2.0}
            MapRoadRule{kind: "tertiary" sort_rank: 390 casing_color: #x4b5765 casing_width: 2.4 center_color: #x7d899a center_width: 1.7}
            MapRoadRule{kind: "residential" sort_rank: 310 casing_color: #x404a57 casing_width: 2.0 center_color: #x677383 center_width: 1.35}
            MapRoadRule{kind: "unclassified" sort_rank: 310 casing_color: #x404a57 casing_width: 2.0 center_color: #x677383 center_width: 1.35}
            MapRoadRule{kind: "living_street" sort_rank: 310 casing_color: #x404a57 casing_width: 2.0 center_color: #x677383 center_width: 1.35}
            MapRoadRule{kind: "service" sort_rank: 240 casing_color: #x3e4753 casing_width: 1.75 center_color: #x5e6a79 center_width: 1.1}
            MapRoadRule{kind: "pedestrian" sort_rank: 240 casing_color: #x3e4753 casing_width: 1.75 center_color: #x5e6a79 center_width: 1.1}
            MapRoadRule{kind: "cycleway" sort_rank: 160 center_color: #x4f5966 center_width: 0.82}
            MapRoadRule{kind: "footway" sort_rank: 160 center_color: #x4f5966 center_width: 0.82}
            MapRoadRule{kind: "path" sort_rank: 160 center_color: #x4f5966 center_width: 0.82}
            MapRoadRule{kind: "steps" sort_rank: 160 center_color: #x4f5966 center_width: 0.82}
            MapRoadRule{kind: "track" sort_rank: 160 center_color: #x4f5966 center_width: 0.82}
            MapRoadRule{kind: "*" sort_rank: 280 casing_color: #x404a57 casing_width: 1.9 center_color: #x606c7b center_width: 1.2}

            MapWaterwayRule{kind: "river" sort_rank: 140 casing_color: #x2f6188 casing_width: 1.83 center_color: #x4f93c8 center_width: 1.55}
            MapWaterwayRule{kind: "canal" sort_rank: 140 casing_color: #x2f6188 casing_width: 1.5 center_color: #x4f93c8 center_width: 1.22}
            MapWaterwayRule{kind: "stream" sort_rank: 140 casing_color: #x2f6188 casing_width: 1.18 center_color: #x4f93c8 center_width: 0.9}
            MapWaterwayRule{kind: "*" sort_rank: 140 casing_color: #x2f6188 casing_width: 1.1 center_color: #x4f93c8 center_width: 0.82}
            MapRailRule{sort_rank: 180 casing_color: #x3f4650 casing_width: 0.96 center_color: #x707783 center_width: 0.62 center_shape_id: 10.0}
        }

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

// --- Draw shaders ---

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

// --- MapView widget ---

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
    #[live(false)]
    dark_theme: bool,
    #[live]
    style_light: MapThemeStyle,
    #[live]
    style_dark: MapThemeStyle,
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
    label_perf: LabelPerfStats,
    #[rust]
    local_source_missing_logged: bool,
    #[rust]
    tile_worker_rx: ToUIReceiver<TileWorkerMessage>,
    #[rust]
    tile_thread_pool: Option<TagThreadPool<TileKey>>,
    #[rust]
    local_requested_tiles: HashSet<TileKey>,
    #[rust]
    local_missing_tiles: HashSet<TileKey>,
    #[rust]
    applied_dark_theme: Option<bool>,
    #[rust]
    style_epoch: u64,
    #[rust]
    compiled_style_light: CompiledMapTheme,
    #[rust]
    compiled_style_dark: CompiledMapTheme,
    #[rust]
    path_glyphs: Vec<PathGlyphInstance>,
    // Scratch buffers reused across frames to avoid per-frame allocations
    #[rust]
    scratch_draw_tiles: Vec<TileKey>,
    #[rust]
    scratch_draw_seen: HashSet<TileKey>,
    #[rust]
    scratch_descendant_tiles: Vec<TileKey>,
    #[rust]
    scratch_candidates: Vec<LabelCandidate>,
    #[rust]
    scratch_accepted_centers: HashMap<String, Vec<Vec2d>>,
    #[rust]
    scratch_accepted_bounds: Vec<Rect>,
    #[rust]
    scratch_accepted_plans: Vec<(f64, usize, usize)>,
    #[rust]
    scratch_screen_path: Vec<Vec2d>,
    #[rust]
    scratch_cumulative: Vec<f64>,
    #[rust]
    scratch_smooth_a: Vec<Vec2d>,
    #[rust]
    scratch_smooth_b: Vec<Vec2d>,
    #[rust]
    prev_status_label_perf: LabelPerfStats,
    #[rust]
    prev_status_counters: (usize, usize, usize, usize, usize, usize),
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

        let previous_light = self.compiled_style_light.clone();
        let previous_dark = self.compiled_style_dark.clone();
        self.rebuild_compiled_styles();
        let styles_changed = previous_light != self.compiled_style_light
            || previous_dark != self.compiled_style_dark;
        if self.style_epoch == 0 {
            self.style_epoch = 1;
        }

        let theme_changed = self.applied_dark_theme != Some(self.dark_theme);
        if theme_changed || styles_changed {
            self.apply_theme_change();
            self.applied_dark_theme = Some(self.dark_theme);
        } else {
            self.apply_theme_palette();
        }

        if self.next_request_id == 0 {
            self.next_request_id = 1;
        }
        ensure_cache_dir();
        if self.status.is_empty() {
            self.status = "Loading Amsterdam tiles from local cache/mbtiles...".to_string();
        }
    }
}

impl Widget for MapView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.handle_tile_worker_messages(cx);
        self.widget_match_event(cx, event, scope);

        if let Event::KeyDown(ke) = event {
            if ke.key_code == KeyCode::KeyT {
                self.set_dark_theme(cx, !self.dark_theme);
            }
        }

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

        self.fill_draw_tile_keys();
        self.scratch_draw_tiles
            .sort_unstable_by_key(|key| (key.z, key.y, key.x));
        // Take draw_tiles out so we can pass &[TileKey] while mutating self for labels
        let draw_tiles = std::mem::take(&mut self.scratch_draw_tiles);

        // Fill pass
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

        // Stroke pass
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

        // Labels
        if view_zoom >= 13.0 {
            self.place_and_draw_labels(cx, &draw_tiles, view_zoom, map_offset, rect);
        } else {
            self.label_perf = LabelPerfStats::default();
        }

        // Put draw_tiles back into scratch buffer (preserves allocation)
        self.scratch_draw_tiles = draw_tiles;

        self.update_status_text();
        // self.draw_text.draw_abs(cx, dvec2(rect.pos.x + 10.0, rect.pos.y + 16.0), &self.status);
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

        // Offload heavy JSON parsing + tessellation to the thread pool
        self.ensure_tile_thread_pool(cx);
        let pool = self.tile_thread_pool.as_ref().unwrap();
        let sender = self.tile_worker_rx.sender();
        let style_epoch = self.style_epoch;
        let theme_style = self.active_style().clone();

        pool.execute_rev(tile_key, move |_tag| {
            match build_tile_buffers_from_body(tile_key, &body, &theme_style) {
                Ok(buffers) => {
                    store_tile_data_cache_on_disk(tile_key, &body);
                    let _ = sender.send(TileWorkerMessage::NetworkTileParsed {
                        style_epoch,
                        tile_key,
                        buffers,
                    });
                }
                Err(err) => {
                    let _ = sender.send(TileWorkerMessage::NetworkTileParseFailed {
                        style_epoch,
                        tile_key,
                        error: err,
                    });
                }
            }
        });
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
        self.mark_tile_failed(
            pending.tile_key,
            &format!(
                "endpoint {} http request error: {:?}",
                pending.endpoint, err
            ),
        );
        self.update_status_text();
        self.redraw(cx);
    }
}

// --- MapView impl ---

impl MapView {
    fn rebuild_compiled_styles(&mut self) {
        self.compiled_style_light = self.style_light.compile();
        self.compiled_style_dark = self.style_dark.compile();
    }

    fn active_style(&self) -> &CompiledMapTheme {
        if self.dark_theme {
            &self.compiled_style_dark
        } else {
            &self.compiled_style_light
        }
    }

    fn normalize_source_mode(&mut self) {
        if self.use_local_mbtiles && self.use_network {
            log!("MapView: both sources enabled; selecting OFFLINE mode (mbtiles only). Set use_local_mbtiles:false for ONLINE mode.");
            self.use_network = false;
        } else if !self.use_local_mbtiles && !self.use_network {
            log!("MapView: no source enabled; selecting OFFLINE mode (mbtiles only).");
            self.use_local_mbtiles = true;
        }
    }

    fn set_dark_theme(&mut self, cx: &mut Cx, dark_theme: bool) {
        if self.dark_theme == dark_theme {
            return;
        }
        self.dark_theme = dark_theme;
        self.apply_theme_change();
        self.applied_dark_theme = Some(self.dark_theme);
        self.update_status_text();
        self.redraw(cx);
    }

    fn apply_theme_change(&mut self) {
        self.style_epoch = self.style_epoch.wrapping_add(1);
        if self.style_epoch == 0 {
            self.style_epoch = 1;
        }
        self.apply_theme_palette();
        self.tiles.clear();
        self.request_to_tile.clear();
        self.local_requested_tiles.clear();
    }

    fn apply_theme_palette(&mut self) {
        let (background, label) = {
            let style = self.active_style();
            (style.background, style.label)
        };
        self.draw_bg.color = background;
        self.draw_label.draw_super.color = label;
        self.draw_text.color = vec4(0.0, 0.0, 0.0, 1.0);
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.draw_bg.redraw(cx);
    }

    fn insert_ready_tile(&mut self, cx: &mut Cx, tile_key: TileKey, buffers: TileBuffers) {
        let fill_geometry = if !buffers.fill_indices.is_empty() && !buffers.fill_vertices.is_empty()
        {
            let geometry = Geometry::new(cx);
            geometry.update(cx, buffers.fill_indices, buffers.fill_vertices);
            Some(geometry)
        } else {
            None
        };

        let stroke_geometry =
            if !buffers.stroke_indices.is_empty() && !buffers.stroke_vertices.is_empty() {
                let geometry = Geometry::new(cx);
                geometry.update(cx, buffers.stroke_indices, buffers.stroke_vertices);
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
                    feature_count: buffers.feature_count,
                    labels: buffers.labels,
                },
                last_used: self.frame_counter,
                attempts: 0,
            },
        );
    }

    fn handle_tile_worker_messages(&mut self, cx: &mut Cx) {
        let mut redraw = false;
        while let Ok(msg) = self.tile_worker_rx.try_recv() {
            match msg {
                TileWorkerMessage::LocalBatchLoaded {
                    style_epoch,
                    requested,
                    loaded,
                } => {
                    if style_epoch != self.style_epoch {
                        for key in &requested {
                            self.local_requested_tiles.remove(key);
                        }
                        continue;
                    }
                    for key in &requested {
                        self.local_requested_tiles.remove(key);
                    }

                    let mut loaded_keys = HashSet::with_capacity(loaded.len());
                    let mut empty_feature_tiles = Vec::<TileKey>::new();
                    for tile in loaded {
                        loaded_keys.insert(tile.tile_key);
                        self.local_missing_tiles.remove(&tile.tile_key);
                        if tile.buffers.feature_count == 0 {
                            empty_feature_tiles.push(tile.tile_key);
                        }
                        self.insert_ready_tile(cx, tile.tile_key, tile.buffers);
                    }
                    if !empty_feature_tiles.is_empty() {
                        empty_feature_tiles.sort_unstable();
                        log!("MapView: local mbtiles loaded {} tile(s) with 0 rendered features sample:{}", empty_feature_tiles.len(), format_tile_key_sample(&empty_feature_tiles, 8));
                    }
                    for key in requested {
                        if loaded_keys.contains(&key) {
                            continue;
                        }
                        self.local_missing_tiles.insert(key);
                        self.tiles.remove(&key);
                    }
                    redraw = true;
                }
                TileWorkerMessage::LocalBatchFailed {
                    style_epoch,
                    requested,
                    error,
                } => {
                    if style_epoch != self.style_epoch {
                        for key in &requested {
                            self.local_requested_tiles.remove(key);
                        }
                        continue;
                    }
                    log!("MapView: local mbtiles load failed: {}", error);
                    for key in requested {
                        self.local_requested_tiles.remove(&key);
                        self.tiles.remove(&key);
                    }
                    redraw = true;
                }
                TileWorkerMessage::NetworkTileParsed {
                    style_epoch,
                    tile_key,
                    buffers,
                } => {
                    if style_epoch != self.style_epoch {
                        continue;
                    }
                    self.insert_ready_tile(cx, tile_key, buffers);
                    redraw = true;
                }
                TileWorkerMessage::NetworkTileParseFailed {
                    style_epoch,
                    tile_key,
                    error,
                } => {
                    if style_epoch != self.style_epoch {
                        continue;
                    }
                    self.mark_tile_failed(tile_key, &format!("parse: {}", error));
                    redraw = true;
                }
            }
        }
        if redraw {
            self.update_status_text();
            self.redraw(cx);
        }
    }

    fn request_visible_tiles_from_local_source(&mut self, _cx: &mut Cx) {
        if !self.use_local_mbtiles {
            return;
        }

        let mbtiles_path = Path::new(LOCAL_MBTILES_PATH);
        if !mbtiles_path.is_file() {
            if !self.local_source_missing_logged {
                log!("MapView: local mbtiles source missing at {} (set use_local_mbtiles: false to disable)", LOCAL_MBTILES_PATH);
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

        let pool = self.tile_thread_pool.as_ref().unwrap();
        let sender = self.tile_worker_rx.sender();
        let requested = missing.clone();
        let mbtiles_path = LOCAL_MBTILES_PATH.to_string();
        let cache_dir = TILE_CACHE_DIR.to_string();
        let style_epoch = self.style_epoch;
        let theme_style = self.active_style().clone();
        let batch_tag = missing[0];

        pool.execute_rev(batch_tag, move |_tag| {
            let result = load_local_tile_batch(
                Path::new(&mbtiles_path),
                Path::new(&cache_dir),
                &requested,
                &theme_style,
            );
            match result {
                Ok(loaded) => {
                    let _ = sender.send(TileWorkerMessage::LocalBatchLoaded {
                        style_epoch,
                        requested,
                        loaded,
                    });
                }
                Err(error) => {
                    let _ = sender.send(TileWorkerMessage::LocalBatchFailed {
                        style_epoch,
                        requested,
                        error,
                    });
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

    fn ensure_tile_thread_pool(&mut self, cx: &mut Cx) {
        if self.tile_thread_pool.is_none() {
            let num_threads = cx.cpu_cores().max(3) - 2;
            self.tile_thread_pool = Some(TagThreadPool::new(cx, num_threads));
        }
    }

    fn ensure_visible_tiles(&mut self, cx: &mut Cx, rect: Rect) {
        self.frame_counter = self.frame_counter.wrapping_add(1);
        self.visible_tiles = self.visible_tile_keys(rect);
        let target_zoom = self.request_zoom_level();

        self.ensure_tile_thread_pool(cx);
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
            .filter(|e| matches!(e.state, TileLoadState::LoadingNetwork))
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

    fn fill_draw_tile_keys(&mut self) {
        self.scratch_draw_tiles.clear();
        self.scratch_draw_seen.clear();

        for i in 0..self.visible_tiles.len() {
            let key = self.visible_tiles[i];
            if self.tile_is_ready(key) {
                if self.scratch_draw_seen.insert(key) {
                    self.scratch_draw_tiles.push(key);
                }
                continue;
            }
            if let Some(draw_key) = self.find_ready_ancestor(key) {
                if self.scratch_draw_seen.insert(draw_key) {
                    self.scratch_draw_tiles.push(draw_key);
                }
                continue;
            }
            self.fill_ready_descendants(key);
            for j in 0..self.scratch_descendant_tiles.len() {
                let draw_key = self.scratch_descendant_tiles[j];
                if self.scratch_draw_seen.insert(draw_key) {
                    self.scratch_draw_tiles.push(draw_key);
                }
            }
        }
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

    fn fill_ready_descendants(&mut self, key: TileKey) {
        self.scratch_descendant_tiles.clear();
        for (candidate, entry) in &self.tiles {
            if !matches!(entry.state, TileLoadState::Ready { .. }) {
                continue;
            }
            if is_descendant_tile(*candidate, key) {
                self.scratch_descendant_tiles.push(*candidate);
            }
        }
    }

    fn request_tile(
        &mut self,
        cx: &mut Cx,
        tile_key: TileKey,
        attempts: u8,
        allow_network: bool,
    ) -> bool {
        if attempts == 0 && !self.use_local_mbtiles {
            let cache_path = tile_data_cache_path_for(tile_key);
            if let Ok(cached_body) = fs::read_to_string(&cache_path) {
                // Offload heavy JSON parsing + tessellation to the thread pool
                self.ensure_tile_thread_pool(cx);
                let pool = self.tile_thread_pool.as_ref().unwrap();
                let sender = self.tile_worker_rx.sender();
                let style_epoch = self.style_epoch;
                let theme_style = self.active_style().clone();
                self.tiles.insert(
                    tile_key,
                    TileEntry {
                        state: TileLoadState::LoadingLocal,
                        last_used: self.frame_counter,
                        attempts: 0,
                    },
                );
                pool.execute_rev(tile_key, move |_tag| {
                    match build_tile_buffers_from_body(tile_key, &cached_body, &theme_style) {
                        Ok(buffers) => {
                            let _ = sender.send(TileWorkerMessage::NetworkTileParsed {
                                style_epoch,
                                tile_key,
                                buffers,
                            });
                        }
                        Err(_err) => {
                            let _ = fs::remove_file(&cache_path);
                            let _ = sender.send(TileWorkerMessage::NetworkTileParseFailed {
                                style_epoch,
                                tile_key,
                                error: String::new(),
                            });
                        }
                    }
                });
                return false;
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

    fn place_and_draw_labels(
        &mut self,
        cx: &mut Cx2d,
        draw_tiles: &[TileKey],
        view_zoom: f64,
        map_offset: Vec2f,
        rect: Rect,
    ) {
        let mut label_perf = LabelPerfStats::default();
        self.collect_label_candidates(draw_tiles, view_zoom, map_offset, rect, &mut label_perf);
        if self.scratch_candidates.is_empty() {
            self.label_perf = label_perf;
            return;
        }
        self.scratch_candidates
            .sort_unstable_by(|a, b| b.score.total_cmp(&a.score));
        let candidate_budget = label_candidate_budget(view_zoom);
        if self.scratch_candidates.len() > candidate_budget {
            self.scratch_candidates.truncate(candidate_budget);
        }
        label_perf.candidates_kept = self.scratch_candidates.len();
        label_perf.shape_budget = label_shape_attempt_budget(view_zoom);

        self.path_glyphs.clear();
        // Clear but retain allocations from previous frames
        for v in self.scratch_accepted_centers.values_mut() {
            v.clear();
        }
        self.scratch_accepted_bounds.clear();
        self.scratch_accepted_plans.clear();

        for candidate_index in 0..self.scratch_candidates.len() {
            let candidate = &self.scratch_candidates[candidate_index];
            let close_repeat = self
                .scratch_accepted_centers
                .get(&candidate.name_key)
                .is_some_and(|centers| {
                    let r2 = candidate.repeat_distance * candidate.repeat_distance;
                    centers.iter().any(|c| {
                        let dx = c.x - candidate.center.x;
                        let dy = c.y - candidate.center.y;
                        dx * dx + dy * dy < r2
                    })
                });
            if close_repeat {
                label_perf.rejected_repeat += 1;
                continue;
            }

            let estimated_width =
                estimate_label_width_pixels(&candidate.text, candidate.font_scale);
            if candidate.path_length < estimated_width + 4.0 {
                label_perf.rejected_pre_short += 1;
                continue;
            }

            if label_perf.shaped_attempts >= label_perf.shape_budget {
                label_perf.rejected_budget +=
                    label_perf.candidates_kept.saturating_sub(candidate_index);
                break;
            }
            label_perf.shaped_attempts += 1;
            // Build placement needs mutable self for draw_label + path_glyphs,
            // but only reads scratch_candidates[candidate_index] immutably.
            // Safe because build_label_placement doesn't touch scratch_candidates.
            let candidate_ptr = &self.scratch_candidates[candidate_index] as *const LabelCandidate;
            let candidate_ref = unsafe { &*candidate_ptr };
            let Some(placement) = self.build_label_placement(cx, candidate_ref) else {
                label_perf.rejected_plan_none += 1;
                continue;
            };
            label_perf.shaped_ok += 1;
            if rect_outside_rect(placement.bounds, rect, LABEL_VIEW_MARGIN) {
                self.path_glyphs.truncate(placement.glyph_start);
                label_perf.rejected_outside += 1;
                continue;
            }
            if self.scratch_accepted_bounds.iter().any(|placed| {
                rects_overlap_with_padding(*placed, placement.bounds, LABEL_COLLISION_PADDING)
            }) {
                self.path_glyphs.truncate(placement.glyph_start);
                label_perf.rejected_collision += 1;
                continue;
            }

            let candidate = &self.scratch_candidates[candidate_index];
            let name_key = &candidate.name_key;
            if let Some(centers) = self.scratch_accepted_centers.get_mut(name_key) {
                centers.push(placement.center);
            } else {
                let key = name_key.clone();
                self.scratch_accepted_centers
                    .entry(key)
                    .or_default()
                    .push(placement.center);
            }
            self.scratch_accepted_bounds.push(placement.bounds);
            let glyph_count = placement.glyph_end - placement.glyph_start;
            label_perf.drawn_labels += 1;
            label_perf.drawn_glyphs += glyph_count;
            let score = candidate.score + candidate.source_rank as f64 * 2.0;
            self.scratch_accepted_plans
                .push((score, placement.glyph_start, placement.glyph_end));
        }

        self.scratch_accepted_plans
            .sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        for i in 0..self.scratch_accepted_plans.len() {
            let (_, start, end) = self.scratch_accepted_plans[i];
            self.draw_label
                .draw_path_glyphs(cx, &self.path_glyphs[start..end]);
        }
        self.label_perf = label_perf;
    }

    fn collect_label_candidates(
        &mut self,
        draw_tiles: &[TileKey],
        view_zoom: f64,
        map_offset: Vec2f,
        rect: Rect,
        label_perf: &mut LabelPerfStats,
    ) {
        // Reuse scratch_candidates: clear but retain per-element heap allocations
        // (String, Vec<Vec2d>) from previous frames so they don't re-allocate.
        for c in self.scratch_candidates.iter_mut() {
            c.text.clear();
            c.name_key.clear();
            c.road_kind.clear();
            c.screen_path.clear();
        }
        let mut write_idx = 0usize;

        for key in draw_tiles {
            label_perf.draw_tiles += 1;
            let Some(entry) = self.tiles.get(key) else {
                continue;
            };
            let TileLoadState::Ready { labels, .. } = &entry.state else {
                continue;
            };
            if labels.is_empty() {
                continue;
            }
            label_perf.tiles_with_labels += 1;
            label_perf.labels_in_tiles += labels.len();
            let scale = 2.0_f64.powf(view_zoom - key.z as f64) as f32;
            let zoom_delta = (view_zoom - key.z as f64).abs();

            for label in labels {
                label_perf.labels_scanned += 1;
                let Some(source_rank) = label_source_rank(&label.source_layer) else {
                    continue;
                };
                let name_key = normalize_label_key(label.text.as_str());
                if name_key.len() < 2 {
                    continue;
                }

                // Build screen_path into scratch buffer, then move it into candidate
                self.scratch_screen_path.clear();
                build_screen_polyline_into(
                    &label.path_points,
                    scale,
                    map_offset,
                    &mut self.scratch_screen_path,
                );
                if self.scratch_screen_path.len() < 2
                    || polyline_outside_rect(&self.scratch_screen_path, rect, LABEL_VIEW_MARGIN)
                {
                    continue;
                }
                self.scratch_cumulative.clear();
                polyline_cumulative_lengths_into(
                    &self.scratch_screen_path,
                    &mut self.scratch_cumulative,
                );
                let path_length = *self.scratch_cumulative.last().unwrap_or(&0.0);
                if path_length < LABEL_MIN_PATH_PIXELS {
                    continue;
                }
                let Some(center) = sample_polyline_point_at_distance(
                    &self.scratch_screen_path,
                    &self.scratch_cumulative,
                    path_length * 0.5,
                ) else {
                    continue;
                };
                if point_outside_rect(center, rect, LABEL_VIEW_MARGIN) {
                    continue;
                }

                let repeat_distance = repeat_distance_for_label(label.priority, source_rank);
                // Use a fixed font_scale per tile zoom level so that labels
                // don't shift along the path during continuous zoom.
                let mut font_scale = 0.92_f32;
                font_scale *= match label.priority {
                    1 => 1.08,
                    2 => 1.0,
                    _ => 0.92,
                };

                let score = source_rank as f64 * 1000.0
                    + (4_u8.saturating_sub(label.priority) as f64) * 120.0
                    + (220.0 - zoom_delta * 65.0)
                    + path_length.min(640.0) * 0.35;

                // Reuse existing candidate slot or push a new one
                if write_idx < self.scratch_candidates.len() {
                    let c = &mut self.scratch_candidates[write_idx];
                    c.text.push_str(&label.text);
                    c.name_key.push_str(&name_key);
                    c.road_kind.push_str(&label.road_kind);
                    c.source_rank = source_rank;
                    c.score = score;
                    c.path_length = path_length;
                    c.center = center;
                    c.repeat_distance = repeat_distance;
                    c.font_scale = font_scale;
                    c.screen_path.extend_from_slice(&self.scratch_screen_path);
                } else {
                    self.scratch_candidates.push(LabelCandidate {
                        text: label.text.clone(),
                        name_key,
                        road_kind: label.road_kind.clone(),
                        source_rank,
                        score,
                        path_length,
                        center,
                        repeat_distance,
                        font_scale,
                        screen_path: self.scratch_screen_path.clone(),
                    });
                }
                write_idx += 1;
                label_perf.candidates += 1;
            }
        }
        self.scratch_candidates.truncate(write_idx);
    }

    fn build_label_placement(
        &mut self,
        cx: &mut Cx2d,
        candidate: &LabelCandidate,
    ) -> Option<PathTextPlacement> {
        if candidate.screen_path.len() < 2 {
            return None;
        }

        // Smooth the candidate's screen_path into scratch_smooth_a,
        // using scratch_smooth_b and scratch_cumulative as temp buffers.
        let mut smooth_a = std::mem::take(&mut self.scratch_smooth_a);
        let mut smooth_b = std::mem::take(&mut self.scratch_smooth_b);
        let mut cum = std::mem::take(&mut self.scratch_cumulative);

        smooth_label_curve_into(
            &candidate.screen_path,
            &mut smooth_a,
            &mut smooth_b,
            &mut cum,
        );

        if smooth_a.len() < 2 {
            self.scratch_smooth_a = smooth_a;
            self.scratch_smooth_b = smooth_b;
            self.scratch_cumulative = cum;
            return None;
        }

        self.draw_label.draw_super.font_scale = candidate.font_scale;
        let run = self
            .draw_label
            .draw_super
            .prepare_single_line_run(cx, candidate.text.as_str());
        let run = match run {
            Some(r) if !r.glyphs.is_empty() => r,
            _ => {
                self.scratch_smooth_a = smooth_a;
                self.scratch_smooth_b = smooth_b;
                self.scratch_cumulative = cum;
                return None;
            }
        };

        // Build cumulative lengths for the smoothed path
        cum.clear();
        polyline_cumulative_lengths_into(&smooth_a, &mut cum);

        let text_width = run.width_in_lpxs;
        let start_distance = choose_label_start_distance(&smooth_a, &cum, text_width as f64);
        let start_distance = match start_distance {
            Some(d) => d,
            None => {
                self.scratch_smooth_a = smooth_a;
                self.scratch_smooth_b = smooth_b;
                self.scratch_cumulative = cum;
                return None;
            }
        };

        let mid_distance = start_distance + text_width as f64 * 0.5;
        let probe_delta = (text_width as f64 * 0.25).clamp(12.0, 42.0);
        let mid_tangent_angle =
            sample_polyline_tangent_angle_raw(&smooth_a, &cum, mid_distance, probe_delta);
        let mid_tangent_angle = match mid_tangent_angle {
            Some(a) => a,
            None => {
                self.scratch_smooth_a = smooth_a;
                self.scratch_smooth_b = smooth_b;
                self.scratch_cumulative = cum;
                return None;
            }
        };
        let reverse = choose_label_reverse(mid_tangent_angle);
        let label_angle_bias = if reverse { std::f32::consts::PI } else { 0.0 };

        let baseline_shift = (run.ascender_in_lpxs + run.descender_in_lpxs)
            * 0.5
            * LABEL_BASELINE_SHIFT_FACTOR as f32;

        let result = self.draw_label.place_text_along_path(
            &run,
            &smooth_a,
            &cum,
            start_distance,
            reverse,
            baseline_shift,
            label_angle_bias,
            LABEL_MAX_GLYPH_TURN_RADIANS,
            LABEL_GLYPH_ANGLE_BLEND,
            candidate.center,
            &mut self.path_glyphs,
        );

        self.scratch_smooth_a = smooth_a;
        self.scratch_smooth_b = smooth_b;
        self.scratch_cumulative = cum;
        result
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

        let counters = (ready, loading, failed, retrying, exhausted, features);
        let lp = self.label_perf;
        // Skip format! if nothing changed since the last call
        if counters == self.prev_status_counters
            && lp == self.prev_status_label_perf
            && !self.status.is_empty()
        {
            return;
        }
        self.prev_status_counters = counters;
        self.prev_status_label_perf = lp;

        self.status = format!(
            "Amsterdam [{}|{}] z{:.2} (req:{})  ready:{}  loading:{}  failed:{}(retry:{} stuck:{})  features:{}  labels(tile:{} scan:{} cand:{}/{} shape:{}/{}(b:{}) draw:{} glyphs:{} rej:r{} ps{} p{} o{} c{} b{})",
            self.source_mode_label(), self.theme_label(), self.view_zoom(), self.request_zoom_level(),
            ready, loading, failed, retrying, exhausted, features,
            lp.labels_in_tiles, lp.labels_scanned, lp.candidates_kept, lp.candidates,
            lp.shaped_ok, lp.shaped_attempts, lp.shape_budget, lp.drawn_labels, lp.drawn_glyphs,
            lp.rejected_repeat, lp.rejected_pre_short, lp.rejected_plan_none,
            lp.rejected_outside, lp.rejected_collision, lp.rejected_budget,
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

    fn theme_label(&self) -> &'static str {
        if self.dark_theme {
            "dark"
        } else {
            "light"
        }
    }
}
