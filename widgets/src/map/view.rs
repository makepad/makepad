use super::geometry::*;
use super::icons::ICON_MIN_ZOOM;
use super::label::*;
use super::overlay::*;
use super::style::*;
use super::tile::*;
use crate::{
    makepad_derive_widget::*, makepad_draw::*, widget::*, DrawRotatedText, DrawVector,
    PathGlyphInstance, PathTextPlacement, PreparedTextRun, WidgetMatchEvent,
};
use makepad_mbtile_reader::MbtilesReader;
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
        map_scale: uniform(vec2(1.0, 1.0))
        map_offset: uniform(vec2(0.0, 0.0))
        tile_fade: uniform(1.0)
        width_correction: uniform(vec4(1.0, 1.0, 1.0, 1.0))
        // Heading-up camera: cos/sin of the screen rotation and its pivot
        // (the view center). Identity when north-up.
        view_rot: uniform(vec2(1.0, 0.0))
        rot_pivot: uniform(vec2(0.0, 0.0))
        // 2.5D camera: x = cos(tilt) screen-y compression, y = screen px of
        // lift per meter of building height (sin(tilt) baked in), z = depth
        // per screen-y of view-space ground position (hardware occlusion
        // for extruded geometry, rotation-proof), w unused.
        tilt_params: uniform(vec4(1.0, 0.0, 0.0, 0.0))

        fragment: fn(){
            self.fb0 = depth_clip(self.v_world_clip, self.pixel() * self.tile_fade * self.fill_pattern(), self.depth_clip)
        }

        vertex: fn() {
            let pos = vec2(self.geom.x, self.geom.y);
            var transformed = pos * self.map_scale + self.map_offset;
            var shape_id = self.geom.shape_id;
            var expanded = 0.0;
            var expand_slack = 0.0;
            // shape >= 100: GPU re-expandable stroke — the position is the
            // centerline anchor, param1/2 the baked half-width offset and
            // param3 the width-growth class. The per-class correction turns
            // the baked width into the width the current view zoom calls
            // for, so stale-bucket tiles stay correct through a zoom.
            if shape_id > 99.5 {
                shape_id = shape_id - 100.0;
                expanded = 1.0;
                var corr = self.width_correction.x;
                if self.geom.param3 > 2.5 {
                    corr = self.width_correction.w;
                } else if self.geom.param3 > 1.5 {
                    corr = self.width_correction.z;
                } else if self.geom.param3 > 0.5 {
                    corr = self.width_correction.y;
                }
                let off = vec2(self.geom.param1, self.geom.param2);
                transformed = transformed + off * self.map_scale * corr;
                expand_slack = length(off) * (corr + 1.0);
            }
            // Heading-up camera: rotate map geometry (and expanded stroke
            // offsets, which are map-space) about the view center.
            let rel = transformed - self.rot_pivot;
            transformed = self.rot_pivot + vec2(
                rel.x * self.view_rot.x - rel.y * self.view_rot.y,
                rel.x * self.view_rot.y + rel.y * self.view_rot.x
            );
            // 2.5D: axonometric tilt compresses screen y about the pivot;
            // building vertices carry their height in meters in param4 and
            // extrude toward screen-top. The pre-tilt (ground) y doubles as
            // the view depth so the depth buffer resolves occlusion.
            let ground_rel_y = transformed.y - self.rot_pivot.y;
            transformed.y = self.rot_pivot.y
                + ground_rel_y * self.tilt_params.x
                - self.geom.param4 * self.tilt_params.y;
            // shape 20: zoom-constant symbol — position is the anchor point,
            // param1/2 the vertex offset in screen px added after the
            // transform. POI symbols stay upright; map-aligned glyphs like
            // oneway arrows (param3 flag) rotate with the camera.
            if shape_id > 19.5 && shape_id < 20.5 {
                var off = vec2(self.geom.param1, self.geom.param2);
                if self.geom.param3 > 0.5 {
                    off = vec2(
                        off.x * self.view_rot.x - off.y * self.view_rot.y,
                        off.x * self.view_rot.y + off.y * self.view_rot.x
                    );
                }
                transformed = transformed + off;
            }

            self.v_tcoord = vec2(self.geom.u, self.geom.v);
            self.v_color = vec4(self.geom.color_r, self.geom.color_g, self.geom.color_b, self.geom.color_a);
            self.v_stroke_mult = self.geom.stroke_mult;
            // stroke distances are tile-local; scale so dash patterns stay in screen px
            self.v_stroke_dist = self.geom.stroke_dist * self.map_scale.x;
            self.v_shape_id = shape_id;
            self.v_param0 = self.geom.param0;
            self.v_param5 = self.geom.param5;

            let grad_type = self.geom.param0;
            if expanded > 0.5 {
                self.v_param1 = 0.0;
                self.v_param2 = 0.0;
                self.v_param3 = 0.0;
                self.v_param4 = 0.0;
            } else if grad_type > 0.5 && grad_type < 1.5 {
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
            } else if shape_id > 0.5 && shape_id < 19.5 {
                let bbox_min = vec2(self.geom.param1, self.geom.param2) * self.map_scale + self.map_offset;
                let bbox_max = vec2(self.geom.param3, self.geom.param4) * self.map_scale + self.map_offset;
                self.v_param1 = bbox_min.x;
                self.v_param2 = bbox_min.y;
                self.v_param3 = bbox_max.x;
                self.v_param4 = bbox_max.y;
            } else if shape_id > 29.5 && shape_id < 32.5 {
                // Pattern fills: anchor the texture to the MAP, not the
                // screen — tile-local position scaled to view px (stable
                // under pan/rotation; rebakes per zoom like carto).
                let pattern_uv = pos * self.map_scale;
                self.v_param1 = pattern_uv.x;
                self.v_param2 = pattern_uv.y;
                self.v_param3 = 0.0;
                self.v_param4 = 0.0;
            } else {
                self.v_param1 = self.geom.param1;
                self.v_param2 = self.geom.param2;
                self.v_param3 = self.geom.param3;
                self.v_param4 = self.geom.param4;
            }

            let shifted = transformed + self.draw_list.view_shift;
            self.v_world = shifted;

            let cr = (self.geom.clip_radius + expand_slack) * max(self.map_scale.x, self.map_scale.y);
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
                // Flat: classic call-order painting. Tilted: self-contained
                // depth — view-ground y dominates, per-pass offset (in w)
                // keeps casing/center/icon layering, baked feature order
                // shrinks to the smallest scale. draw_call.zbias would
                // otherwise grow with call count and beat small lifts.
                self.draw_depth + self.tilt_params.w
                    + mix(
                        self.draw_call.zbias + self.geom.zbias,
                        self.geom.param5 + ground_rel_y * self.tilt_params.z,
                        sign(self.tilt_params.z)
                    )
                1.
            );
            self.v_world_clip = world;
            self.vertex_pos = self.draw_pass.camera_projection * (self.draw_pass.camera_view * world)
        }

        fill_pattern: fn() {
            // 30: small staggered dot stipple (courtyard gardens).
            if self.v_shape_id > 29.5 && self.v_shape_id < 30.5 {
                let uv = vec2(self.v_param1, self.v_param2)
                let period = 5.0
                let row = floor(uv.y / period)
                let sx = uv.x + fract(row * 0.5) * period
                let cell = fract(vec2(sx, uv.y) / period) - vec2(0.5, 0.5)
                let d = length(cell) * period
                let dot = 1.0 - smoothstep(0.55, 1.0, d)
                let f = 1.0 - 0.14 * dot
                return vec4(f, f, f, 1.0)
            }
            // 31: diagonal hatch (playgrounds).
            if self.v_shape_id > 30.5 && self.v_shape_id < 31.5 {
                let uv = vec2(self.v_param1, self.v_param2)
                let band = fract((uv.x + uv.y) / 9.0)
                let line = 1.0 - smoothstep(0.10, 0.20, abs(band - 0.5))
                let f = 1.0 - 0.12 * line
                return vec4(f, f, f, 1.0)
            }
            // 32: staggered open circles (woods, cemeteries — tree rings).
            if self.v_shape_id > 31.5 && self.v_shape_id < 32.5 {
                let uv = vec2(self.v_param1, self.v_param2)
                let period = 12.0
                let row = floor(uv.y / period)
                let sx = uv.x + fract(row * 0.5) * period
                let cell = fract(vec2(sx, uv.y) / period) - vec2(0.5, 0.5)
                let d = length(cell) * period
                let ring = 1.0 - smoothstep(0.45, 0.85, abs(d - 2.4))
                let f = 1.0 - 0.15 * ring
                return vec4(f, f, f, 1.0)
            }
            return vec4(1.0, 1.0, 1.0, 1.0)
        }

        get_stroke_mask: fn() {
            if self.v_shape_id > 9.5 && self.v_shape_id < 10.5 {
                return self.dash(3.2, 2.4)
            }
            if self.v_shape_id > 10.5 && self.v_shape_id < 11.5 {
                return self.dash(2.0, 3.0)
            }
            if self.v_shape_id > 11.5 && self.v_shape_id < 12.5 {
                return self.dash(8.0, 8.0)
            }
            return 1.0
        }
    }

    mod.widgets.MapViewBase = #(MapView::register_widget(vm))

    mod.widgets.MapView = set_type_default() do mod.widgets.MapViewBase{
        width: Fill
        height: Fill
        center_lon: 4.8779
        center_lat: 52.3757
        zoom: 17.0
        min_zoom: 11.0
        max_zoom: 19.0
        dark_theme: false
        use_network: false
        use_local_mbtiles: true
        // openstreetmap-carto palette; road widths are carto's z14 stops in
        // screen px, scaled per view-zoom bucket by zoom_width_mult().
        style_light: MapThemeStyle{
            background: #xf2efe9
            status_text: #x444444
            label: #x000000

            MapFillRule{group: "building" color: #xd9d0c9}
            MapFillRule{group: "building_outline" color: #xb5aa9b}
            MapFillRule{group: "street_area" color: #xdddde8}
            MapFillRule{group: "bridge_area" color: #xb8b8b8}
            MapFillRule{group: "water" color: #xaad3df}
            MapFillRule{group: "landuse" value: "residential" color: #xe0dfdf}
            MapFillRule{group: "landuse" value: "commercial" color: #xf2dad9}
            MapFillRule{group: "landuse" value: "retail" color: #xffd6d1}
            MapFillRule{group: "landuse" value: "industrial" color: #xebdbe8}
            MapFillRule{group: "landuse" value: "forest" color: #xadd19e}
            MapFillRule{group: "landuse" value: "grass" color: #xcdebb0}
            MapFillRule{group: "landuse" value: "meadow" color: #xcdebb0}
            MapFillRule{group: "landuse" value: "farmland" color: #xeef0d5}
            MapFillRule{group: "landuse" value: "railway" color: #xece7f1}
            MapFillRule{group: "landuse" value: "cemetery" color: #xaacbaf}
            MapFillRule{group: "landuse" value: "sand" color: #xf2e9cf}
            MapFillRule{group: "landuse" value: "*" color: #xe8e7e2}
            MapFillRule{group: "leisure" value: "park" color: #xc8facc}
            MapFillRule{group: "leisure" value: "garden" color: #xcdebb0}
            MapFillRule{group: "leisure" value: "golf_course" color: #xdef6c0}
            MapFillRule{group: "leisure" value: "pitch" color: #x88e0be}
            MapFillRule{group: "leisure" value: "*" color: #xc8facc}

            MapRoadRule{kind: "motorway" sort_rank: 700 casing_color: #xdc2a67 casing_width: 7.2 center_color: #xe892a2 center_width: 6.0}
            MapRoadRule{kind: "trunk" sort_rank: 640 casing_color: #xc84e2f casing_width: 7.2 center_color: #xf9b29c center_width: 6.0}
            MapRoadRule{kind: "primary" sort_rank: 560 casing_color: #xa06b00 casing_width: 6.4 center_color: #xfcd6a4 center_width: 5.0}
            MapRoadRule{kind: "secondary" sort_rank: 470 casing_color: #x707d05 casing_width: 6.4 center_color: #xf7fabf center_width: 5.0}
            MapRoadRule{kind: "busway" sort_rank: 470 casing_color: #x707d05 casing_width: 6.4 center_color: #xf7fabf center_width: 5.0}
            MapRoadRule{kind: "tertiary" sort_rank: 390 casing_color: #x8f8f8f casing_width: 6.2 center_color: #xffffff center_width: 5.0}
            MapRoadRule{kind: "residential" sort_rank: 310 casing_color: #xbbbbbb casing_width: 4.2 center_color: #xffffff center_width: 3.0}
            MapRoadRule{kind: "unclassified" sort_rank: 310 casing_color: #xbbbbbb casing_width: 4.2 center_color: #xffffff center_width: 3.0}
            MapRoadRule{kind: "living_street" sort_rank: 310 casing_color: #xbbbbbb casing_width: 4.0 center_color: #xededed center_width: 3.0}
            MapRoadRule{kind: "service" sort_rank: 240 casing_color: #xbbbbbb casing_width: 3.0 center_color: #xffffff center_width: 2.0}
            MapRoadRule{kind: "pedestrian" sort_rank: 240 casing_color: #x999999 casing_width: 4.0 center_color: #xdddde8 center_width: 3.0}
            MapRoadRule{kind: "pedestrian" sort_rank: 300 casing_color: #xb5b5b5 casing_width: 4.0 center_color: #xfdfdfd center_width: 2.8 min_zoom: 14.0}
            MapRoadRule{kind: "cycleway" sort_rank: 160 center_color: #x6262ff center_width: 0.9 center_shape_id: 10.0 min_zoom: 14.0}
            MapRoadRule{kind: "footway" sort_rank: 160 center_color: #xaaa8a5 center_width: 0.9 center_shape_id: 10.0 min_zoom: 15.0}
            MapRoadRule{kind: "path" sort_rank: 160 center_color: #xaaa8a5 center_width: 0.8 center_shape_id: 10.0 min_zoom: 15.0}
            MapRoadRule{kind: "steps" sort_rank: 160 center_color: #xaaa8a5 center_width: 2.0 center_shape_id: 10.0 min_zoom: 15.0}
            MapRoadRule{kind: "track" sort_rank: 160 center_color: #xaaa8a5 center_width: 1.0 center_shape_id: 10.0 min_zoom: 14.0}
            MapRoadRule{kind: "*" sort_rank: 280 casing_color: #xbbbbbb casing_width: 3.6 center_color: #xffffff center_width: 2.5}

            MapWaterwayRule{kind: "river" sort_rank: 140 center_color: #xaad3df center_width: 4.0}
            MapWaterwayRule{kind: "canal" sort_rank: 140 center_color: #xaad3df center_width: 3.0 min_zoom: 12.0}
            MapWaterwayRule{kind: "stream" sort_rank: 140 center_color: #xaad3df center_width: 1.4 min_zoom: 13.0}
            MapWaterwayRule{kind: "*" sort_rank: 140 center_color: #xaad3df center_width: 1.2 min_zoom: 13.0}
            MapRailRule{sort_rank: 710 center_color: #x6e6e6e center_width: 1.0}
        }
        style_dark: MapThemeStyle{
            background: #x161b22
            status_text: #xb2c7d8
            label: #xe5eaf1
            label_halo: #x161b22

            MapFillRule{group: "building" color: #x383d46}
            MapFillRule{group: "building_outline" color: #x262a31}
            MapFillRule{group: "street_area" color: #x3a3f4a}
            MapFillRule{group: "bridge_area" color: #x3a3f47}
            MapFillRule{group: "water" color: #x204f74}
            MapFillRule{group: "landuse" value: "residential" color: #x2a2f36}
            MapFillRule{group: "landuse" value: "commercial" color: #x30343b}
            MapFillRule{group: "landuse" value: "retail" color: #x30343b}
            MapFillRule{group: "landuse" value: "industrial" color: #x282c32}
            MapFillRule{group: "landuse" value: "forest" color: #x243629}
            MapFillRule{group: "landuse" value: "grass" color: #x2a3c2d}
            MapFillRule{group: "landuse" value: "meadow" color: #x2a3c2d}
            MapFillRule{group: "landuse" value: "farmland" color: #x2a3c2d}
            MapFillRule{group: "landuse" value: "railway" color: #x2f2b36}
            MapFillRule{group: "landuse" value: "cemetery" color: #x2b3a2f}
            MapFillRule{group: "landuse" value: "sand" color: #x3a362c}
            MapFillRule{group: "landuse" value: "*" color: #x2d3239}
            MapFillRule{group: "leisure" value: "park" color: #x2f4a34}
            MapFillRule{group: "leisure" value: "garden" color: #x2f4a34}
            MapFillRule{group: "leisure" value: "golf_course" color: #x2f4a34}
            MapFillRule{group: "leisure" value: "pitch" color: #x32553a}
            MapFillRule{group: "leisure" value: "*" color: #x2b4230}

            MapRoadRule{kind: "motorway" sort_rank: 700 casing_color: #x8f6937 casing_width: 7.2 center_color: #xd29b54 center_width: 6.0}
            MapRoadRule{kind: "trunk" sort_rank: 640 casing_color: #x8c7141 casing_width: 7.2 center_color: #xc8a561 center_width: 6.0}
            MapRoadRule{kind: "primary" sort_rank: 560 casing_color: #x706857 casing_width: 6.4 center_color: #xb9aa86 center_width: 5.0}
            MapRoadRule{kind: "secondary" sort_rank: 470 casing_color: #x556170 casing_width: 6.4 center_color: #x95a1b1 center_width: 5.0}
            MapRoadRule{kind: "busway" sort_rank: 470 casing_color: #x556170 casing_width: 6.4 center_color: #x95a1b1 center_width: 5.0}
            MapRoadRule{kind: "tertiary" sort_rank: 390 casing_color: #x4b5765 casing_width: 6.2 center_color: #x7d899a center_width: 5.0}
            MapRoadRule{kind: "residential" sort_rank: 310 casing_color: #x404a57 casing_width: 4.2 center_color: #x677383 center_width: 3.0}
            MapRoadRule{kind: "unclassified" sort_rank: 310 casing_color: #x404a57 casing_width: 4.2 center_color: #x677383 center_width: 3.0}
            MapRoadRule{kind: "living_street" sort_rank: 310 casing_color: #x404a57 casing_width: 4.0 center_color: #x677383 center_width: 3.0}
            MapRoadRule{kind: "service" sort_rank: 240 casing_color: #x3e4753 casing_width: 3.0 center_color: #x5e6a79 center_width: 2.0}
            MapRoadRule{kind: "pedestrian" sort_rank: 240 casing_color: #x3e4753 casing_width: 4.0 center_color: #x5e6a79 center_width: 3.0}
            MapRoadRule{kind: "pedestrian" sort_rank: 300 casing_color: #x3c424a casing_width: 4.0 center_color: #x272b31 center_width: 2.8 min_zoom: 14.0}
            MapRoadRule{kind: "cycleway" sort_rank: 160 center_color: #x4f5966 center_width: 0.9 center_shape_id: 10.0 min_zoom: 14.0}
            MapRoadRule{kind: "footway" sort_rank: 160 center_color: #x4f5966 center_width: 0.9 center_shape_id: 10.0 min_zoom: 15.0}
            MapRoadRule{kind: "path" sort_rank: 160 center_color: #x4f5966 center_width: 0.8 center_shape_id: 10.0 min_zoom: 15.0}
            MapRoadRule{kind: "steps" sort_rank: 160 center_color: #x4f5966 center_width: 2.0 center_shape_id: 10.0 min_zoom: 15.0}
            MapRoadRule{kind: "track" sort_rank: 160 center_color: #x4f5966 center_width: 1.0 center_shape_id: 10.0 min_zoom: 14.0}
            MapRoadRule{kind: "*" sort_rank: 280 casing_color: #x404a57 casing_width: 3.6 center_color: #x606c7b center_width: 2.5}

            MapWaterwayRule{kind: "river" sort_rank: 140 center_color: #x204f74 center_width: 4.0}
            MapWaterwayRule{kind: "canal" sort_rank: 140 center_color: #x204f74 center_width: 3.0 min_zoom: 12.0}
            MapWaterwayRule{kind: "stream" sort_rank: 140 center_color: #x204f74 center_width: 1.4 min_zoom: 13.0}
            MapWaterwayRule{kind: "*" sort_rank: 140 center_color: #x204f74 center_width: 1.2 min_zoom: 13.0}
            MapRailRule{sort_rank: 710 center_color: #x8a919d center_width: 1.0}
        }

        draw_bg +: {
            color: #xf2efe9
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

/// Frames after the last zoom change before stale-bucket tiles restyle
/// (~0.3s at 60fps).
const ZOOM_SETTLE_SECONDS: f64 = 0.08;

/// Accumulated pan (screen px) before labels are re-placed; must stay under
/// LABEL_VIEW_MARGIN so cached placements keep covering the viewport edge.
const LABEL_REPLACE_PAN_PX: f64 = 48.0;
/// Minimum frames between full label re-placements while the cached
/// placement is still usable (a full place costs up to ~20ms — 2-3 dropped
/// frames at 120Hz — and tile arrivals during panning invalidated the cache
/// almost every other frame).
const LABEL_REPLACE_MIN_SECONDS: f64 = 0.12;
/// Cross-fade duration when a tile's new geometry replaces the old.
const TILE_FADE_SECONDS: f64 = 0.25;
/// Hard time budget for one placement pass; labels that don't make it are
/// picked up by the next re-place.
const LABEL_PLACE_BUDGET_MS: f64 = 7.0;

// --- Actions ---

/// Widget actions emitted by MapView; the app layer builds search, routing
/// and navigation UX on top of these plus the camera/overlay API.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum MapViewAction {
    /// Camera settled after a gesture, fly-to or programmatic move.
    ViewportChanged {
        lon: f64,
        lat: f64,
        zoom: f64,
    },
    /// Finger up without drag or long-press, not on a marker.
    Tapped {
        lon: f64,
        lat: f64,
        abs: Vec2d,
    },
    LongPressed {
        lon: f64,
        lat: f64,
        abs: Vec2d,
    },
    MarkerClicked {
        id: u64,
    },
    #[default]
    None,
}

/// Animated camera flight (zoom-out-then-in arc when the target is far).
#[derive(Clone, Copy)]
struct FlyTo {
    started: std::time::Instant,
    duration: f64,
    from_center: Vec2d,
    to_center: Vec2d,
    from_zoom: f64,
    to_zoom: f64,
    arc: f64,
}

// --- Draw shaders ---

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawMapVector {
    #[deref]
    pub draw_super: DrawVector,
    #[rust(vec2(1.0, 1.0))]
    pub map_scale: Vec2f,
    #[rust(vec2(0.0, 0.0))]
    pub map_offset: Vec2f,
    #[rust(1.0)]
    pub tile_fade: f32,
}

impl DrawMapVector {
    #[allow(clippy::too_many_arguments)]
    fn draw_geometry(
        &mut self,
        cx: &mut Cx2d,
        geometry_id: GeometryId,
        map_scale: Vec2f,
        map_offset: Vec2f,
        fade: f32,
        width_correction: [f32; 4],
        view_rot: [f32; 2],
        rot_pivot: [f32; 2],
        tilt_params: [f32; 4],
    ) {
        self.map_scale = map_scale;
        self.map_offset = map_offset;
        self.tile_fade = fade;
        self.draw_super
            .draw_vars
            .set_uniform(cx.cx, live_id!(tile_fade), &[fade]);
        self.draw_super.draw_vars.set_uniform(
            cx.cx,
            live_id!(map_scale),
            &[map_scale.x, map_scale.y],
        );
        self.draw_super.draw_vars.set_uniform(
            cx.cx,
            live_id!(map_offset),
            &[map_offset.x, map_offset.y],
        );
        self.draw_super.draw_vars.set_uniform(
            cx.cx,
            live_id!(width_correction),
            &width_correction,
        );
        self.draw_super
            .draw_vars
            .set_uniform(cx.cx, live_id!(view_rot), &view_rot);
        self.draw_super
            .draw_vars
            .set_uniform(cx.cx, live_id!(rot_pivot), &rot_pivot);
        self.draw_super
            .draw_vars
            .set_uniform(cx.cx, live_id!(tilt_params), &tilt_params);
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
    #[uid]
    uid: WidgetUid,
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
    #[live(19.0)]
    max_zoom: f64,
    /// Map bearing that points up, in degrees (0 = north-up). Drives the
    /// heading-up navigation camera.
    #[live(0.0)]
    rotation: f64,
    /// Axonometric camera tilt in degrees (0 = top-down). Compresses the
    /// screen y about the view center and lifts 2.5D building geometry.
    #[live(0.0)]
    tilt: f64,
    /// Bake extruded, shaded buildings from the detail archive (needs
    /// `detail_mbtiles_path`).
    #[live(false)]
    buildings_3d: bool,
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
    /// Overrides the built-in LOCAL_MBTILES_PATH when non-empty, so each app
    /// can point its MapView at its own tile archive.
    #[live]
    mbtiles_path: String,
    /// Semicolon-separated list of geodata overlay mbtiles (layers.md
    /// track: chargers, transit, nature, districts…). Set at runtime via
    /// set_overlay_paths; tiles rebuild on change.
    #[live]
    overlay_mbtiles_paths: String,
    /// Optional all-tag detail archive (pbf-detail output) composed over the
    /// base for micro-POI symbols: trees, benches, bins, artwork…
    #[live]
    detail_mbtiles_path: String,
    /// Declared minzoom/maxzoom of the active archive (from its metadata
    /// table). Single-zoom detail archives (minzoom=maxzoom=14) must not be
    /// probed at z13/z12 — those tiles cannot exist.
    #[rust]
    local_source_zoom_range: Option<(u32, u32)>,
    #[rust]
    local_source_zoom_range_path: Option<String>,
    /// True when the metadata read ran while the archive file existed.
    #[rust]
    local_source_zoom_range_checked: bool,

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
    local_requested_tiles: HashMap<TileKey, u64>,
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
    scratch_accepted_plans: Vec<(f64, usize, usize, u8)>,
    // Labels drawn last frame (hashed name+position key); kept to stabilize
    // placement while panning instead of flickering between candidates.
    #[rust]
    prev_label_keys: HashSet<u64>,
    #[rust]
    scratch_accepted_hashes: Vec<u64>,
    // Frame of the last zoom change; zoom-bucket restyles are deferred until
    // the gesture settles so widths don't flicker mid-zoom.
    #[rust]
    last_zoom_change_frame: u64,
    #[rust]
    last_zoom_change_time: Option<std::time::Instant>,
    #[rust]
    zoom_settle_timer: Timer,
    #[rust]
    tile_fade_timer: Timer,
    // Label placement cache: while panning at the same zoom over the same
    // tiles, last placement's glyphs are redrawn shifted by the pan delta
    // instead of re-scanning/re-shaping/re-colliding thousands of labels.
    #[rust]
    label_cache_valid: bool,
    #[rust]
    label_cache_offset: Vec2d,
    #[rust]
    label_cache_zoom: f64,
    #[rust]
    label_cache_rotation: f64,
    #[rust]
    label_cache_tilt: f64,
    #[rust]
    label_cache_tiles: Vec<TileKey>,
    #[rust]
    label_cache_generation: u64,
    #[rust]
    tiles_generation: u64,
    #[rust]
    last_full_place_time: Option<std::time::Instant>,
    #[rust]
    needs_label_followup: bool,
    // Shaped text runs keyed by (text hash, len, quantized font_scale bits);
    // shaping dominates label placement cost.
    #[rust]
    shaped_runs: HashMap<(u64, u32, u32), Option<PreparedTextRun>>,
    // Finished tile buffers waiting for GPU upload; drained a couple per
    // frame so a 10-tile rebuild batch doesn't stall a single frame with
    // hundreds of MB of buffer creation/upload.
    #[rust]
    pending_ready_tiles: Vec<(TileKey, TileBuffers)>,
    #[rust]
    last_tile_upload_frame: u64,
    // Frame-time instrumentation, aggregated to local/map_perf.log.
    #[rust]
    perf_frames: u32,
    #[rust]
    perf_ms_total: f64,
    #[rust]
    perf_ms_geo: f64,
    #[rust]
    perf_ms_labels: f64,
    #[rust]
    perf_ms_max: f64,
    #[rust]
    perf_label_full_places: u32,
    #[rust]
    perf_last_frame: Option<std::time::Instant>,
    #[rust]
    perf_ms_gap_max: f64,
    #[rust]
    perf_gap_sum: f64,
    #[rust]
    perf_gap_count: u32,
    #[rust]
    perf_gaps_over_12ms: u32,
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

    // --- Interaction layer (overlay + camera API) ---
    #[live]
    draw_overlay: DrawVector,
    #[rust]
    overlay: MapOverlayState,
    #[rust]
    fly: Option<FlyTo>,
    #[rust]
    fly_timer: Timer,
    #[rust]
    gesture_panned: bool,
    /// Right-button / Option-drag camera gesture: (start abs, start
    /// rotation, start tilt).
    #[rust]
    rotate_drag: Option<(Vec2d, f64, f64)>,
    #[rust]
    last_tap_count: u32,
    #[rust]
    pending_viewport_changed: bool,
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

        if self.tile_fade_timer.is_event(event).is_some() {
            self.redraw(cx);
            if self.tiles.values().any(|entry| entry.fade.is_some()) {
                self.tile_fade_timer = cx.start_timeout(0.016);
            }
        }
        if self.zoom_settle_timer.is_event(event).is_some() {
            self.redraw(cx);
            if self.pending_viewport_changed {
                self.pending_viewport_changed = false;
                self.sync_camera_fields();
                self.emit_viewport_changed(cx);
            }
            if self.needs_label_followup || !self.pending_ready_tiles.is_empty() {
                self.zoom_settle_timer = cx.start_timeout(0.08);
            }
        }

        if let Event::KeyDown(ke) = event {
            if ke.key_code == KeyCode::KeyT {
                self.set_dark_theme(cx, !self.dark_theme);
            }
        }

        if self.fly_timer.is_event(event).is_some() {
            self.tick_fly(cx);
        }

        // Respect the handled flag (no capture overload): floating UI panels
        // drawn on top of the map must win the hit test (EventOrder::Up
        // dispatches them first).
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerDown(fe) => {
                let rotate_gesture = fe
                    .device
                    .mouse_button()
                    .is_some_and(|button| button.is_secondary())
                    || (fe.is_primary_hit() && fe.modifiers.alt);
                if rotate_gesture {
                    // Right-drag (or Option-drag): horizontal rotates the
                    // camera, vertical tilts it.
                    self.fly = None;
                    self.rotate_drag = Some((fe.abs, self.rotation, self.tilt));
                } else if fe.is_primary_hit() {
                    self.fly = None;
                    self.gesture_panned = false;
                    self.drag_start_abs = Some(fe.abs);
                    self.drag_start_center_norm = self.center_norm;
                    self.last_tap_count = fe.tap_count;
                    cx.set_cursor(MouseCursor::Grabbing);
                }
            }
            Hit::FingerMove(fe) => {
                if let Some((start_abs, start_rotation, start_tilt)) = self.rotate_drag {
                    let delta = fe.abs - start_abs;
                    self.rotation = (start_rotation - delta.x * 0.35).rem_euclid(360.0);
                    self.tilt = (start_tilt + delta.y * 0.25).clamp(0.0, 65.0);
                    self.redraw(cx);
                } else if let Some(start_abs) = self.drag_start_abs {
                    let delta = fe.abs - start_abs;
                    if delta.length() > 6.0 {
                        self.gesture_panned = true;
                    }
                    // Screen drag maps to a world pan through the inverse of
                    // the heading-up rotation and camera tilt.
                    let world_delta = self.screen_delta_to_world(delta);
                    let world_size = tile_world_size_zoom(self.view_zoom());
                    self.center_norm = self.drag_start_center_norm
                        - dvec2(world_delta.x / world_size, world_delta.y / world_size);
                    self.wrap_and_clamp_center();
                    self.redraw(cx);
                }
            }
            Hit::FingerLongPress(lp) => {
                // Long press cancels the pan gesture and reports map coords.
                self.drag_start_abs = None;
                let (lon, lat) = self.screen_to_lon_lat(lp.abs);
                cx.widget_action(self.uid, MapViewAction::LongPressed { lon, lat, abs: lp.abs });
            }
            Hit::FingerUp(fe) => {
                if self.rotate_drag.take().is_some() {
                    self.sync_camera_fields();
                    self.emit_viewport_changed(cx);
                    return;
                }
                self.drag_start_abs = None;
                cx.set_cursor(MouseCursor::Grab);
                if fe.is_primary_hit() && fe.was_tap() {
                    if self.last_tap_count >= 2 {
                        // Double-click acts as the long-press (mouse holds
                        // don't synthesize FingerLongPress on desktop).
                        let (lon, lat) = self.screen_to_lon_lat(fe.abs);
                        cx.widget_action(
                            self.uid,
                            MapViewAction::LongPressed { lon, lat, abs: fe.abs },
                        );
                    } else if let Some(id) = self.overlay.marker_at(&self.overlay_camera(), fe.abs)
                    {
                        cx.widget_action(self.uid, MapViewAction::MarkerClicked { id });
                    } else {
                        let (lon, lat) = self.screen_to_lon_lat(fe.abs);
                        cx.widget_action(self.uid, MapViewAction::Tapped { lon, lat, abs: fe.abs });
                    }
                } else if self.gesture_panned {
                    self.gesture_panned = false;
                    self.sync_camera_fields();
                    self.emit_viewport_changed(cx);
                }
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
                self.fly = None;
                self.zoom_with_anchor(cx, scroll, fs.abs);
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let perf_start = std::time::Instant::now();
        if let Some(last_frame) = self.perf_last_frame {
            let gap_ms = last_frame.elapsed().as_secs_f64() * 1000.0;
            self.perf_ms_gap_max = self.perf_ms_gap_max.max(gap_ms);
            // only count gaps from continuous animation, not idle pauses
            if gap_ms < 100.0 {
                self.perf_gap_sum += gap_ms;
                self.perf_gap_count += 1;
                if gap_ms > 12.0 {
                    self.perf_gaps_over_12ms += 1;
                }
            }
        }
        self.perf_last_frame = Some(perf_start);

        let rect = cx.walk_turtle(walk);
        self.view_rect = rect;
        self.draw_bg.draw_abs(cx, rect);
        self.ensure_visible_tiles(cx, rect);

        let view_zoom = self.view_zoom();
        let world_size = tile_world_size_zoom(view_zoom);
        let center_world = self.center_norm * world_size;
        // Keep the global offset in f64; geometry is tile-local, so the only
        // f32 quantities the GPU sees are small (tile-local coords and a
        // screen-magnitude per-tile offset).
        let map_offset = dvec2(
            rect.pos.x + rect.size.x * 0.5 - center_world.x,
            rect.pos.y + rect.size.y * 0.5 - center_world.y,
        );

        let (rot_cos, rot_sin) = self.screen_rotation();
        let rot_pivot = rect.pos + rect.size * 0.5;
        let view_rot_uniform = [rot_cos as f32, rot_sin as f32];
        let rot_pivot_uniform = [rot_pivot.x as f32, rot_pivot.y as f32];
        let tilt_rad = self.tilt.clamp(0.0, 65.0).to_radians();
        let px_per_meter = {
            let (_, lat) = normalized_to_lon_lat(self.center_norm);
            world_size / (40_075_016.686 * lat.to_radians().cos())
        };
        // Tilted map depth lives in a negative domain well below every UI
        // element, so panels/labels/overlay drawn later always win by call
        // order. Within the map, view-ground y dominates for occlusion and
        // the baked sort-rank micro-depth (param5) resolves overlapping
        // layers at the same ground pixel without depth-precision flicker.
        let tilt_uniform = if tilt_rad > 1e-4 {
            [
                tilt_rad.cos() as f32,
                (px_per_meter * tilt_rad.sin()) as f32,
                0.01,
                -24.0,
            ]
        } else {
            // Flat mode stays byte-identical to the classic paint order.
            [tilt_rad.cos() as f32, 0.0, 0.0, 0.0]
        };

        self.fill_draw_tile_keys();
        // Take draw_tiles out so we can pass &[TileKey] while mutating self for labels
        let mut draw_tiles = std::mem::take(&mut self.scratch_draw_tiles);
        // Tiles still fading in from empty draw LAST (on top): their old-zoom
        // stand-ins painted beneath them make zoom transitions a real
        // cross-fade instead of a flash of background color.
        draw_tiles.sort_unstable_by_key(|key| {
            (
                self.tile_fading_from_empty(*key) as u8,
                key.z,
                key.y,
                key.x,
            )
        });
        let draw_tiles = draw_tiles;

        // Four global passes (carto layer order): every tile's fills, then
        // every tile's road casings, then road centers, then POI symbols.
        // Casings interleaved per tile would stamp over neighbor tiles' road
        // interiors in the clip-padding overlap at tile seams.
        for pass in 0..4 {
            for key in &draw_tiles {
                let Some(entry) = self.tiles.get(key) else {
                    continue;
                };
                let TileLoadState::Ready {
                    fill_geometry,
                    casing_geometry,
                    stroke_geometry,
                    icon_geometry,
                    ..
                } = &entry.state
                else {
                    continue;
                };
                // Stale higher-bucket tiles keep their baked symbols until the
                // rebuild lands; hide the pass immediately on zoom-out instead.
                if pass == 3 && view_zoom < ICON_MIN_ZOOM as f64 - 0.25 {
                    continue;
                }
                let geometry = match pass {
                    0 => fill_geometry,
                    1 => casing_geometry,
                    2 => stroke_geometry,
                    _ => icon_geometry,
                };
                let scale = 2.0_f64.powf(view_zoom - key.z as f64);
                let tile_offset = map_offset
                    + dvec2(
                        key.x as f64 * TILE_SIZE * scale,
                        key.y as f64 * TILE_SIZE * scale,
                    );
                let map_scale = Vec2f {
                    x: scale as f32,
                    y: scale as f32,
                };
                let screen_offset = Vec2f {
                    x: tile_offset.x as f32,
                    y: tile_offset.y as f32,
                };
                let mut fade_alpha = 1.0_f32;
                if let Some(fade) = &entry.fade {
                    fade_alpha = ((fade.started.elapsed().as_secs_f64() / TILE_FADE_SECONDS)
                        as f32)
                        .clamp(0.0, 1.0);
                    let outgoing = match pass {
                        0 => &fade.fill_geometry,
                        1 => &fade.casing_geometry,
                        2 => &fade.stroke_geometry,
                        _ => &fade.icon_geometry,
                    };
                    if let Some(outgoing) = outgoing {
                        let outgoing_id = outgoing.geometry_id();
                        self.draw_map.draw_geometry(
                            cx,
                            outgoing_id,
                            map_scale,
                            screen_offset,
                            1.0,
                            stroke_width_correction(fade.bucket, view_zoom),
                            view_rot_uniform,
                            rot_pivot_uniform,
                            tilt_uniform,
                        );
                    }
                }
                let Some(geometry) = geometry else {
                    continue;
                };
                let geometry_id = geometry.geometry_id();
                self.draw_map.draw_geometry(
                    cx,
                    geometry_id,
                    map_scale,
                    screen_offset,
                    fade_alpha,
                    stroke_width_correction(entry.bucket, view_zoom),
                    view_rot_uniform,
                    rot_pivot_uniform,
                    tilt_uniform,
                );
            }
        }

        let geo_ms = perf_start.elapsed().as_secs_f64() * 1000.0;

        // Labels
        let labels_start = std::time::Instant::now();
        // No global zoom gate: place labels carry the map from z4 (cities)
        // outward, streets/water/nature take over as their own per-kind
        // gates open. The old `view_zoom >= 13` guard predated place
        // labels and blanked EVERYTHING when zoomed out.
        let full_place =
            self.place_and_draw_labels(cx, &draw_tiles, view_zoom, map_offset, rect);
        let labels_ms = labels_start.elapsed().as_secs_f64() * 1000.0;

        // Put draw_tiles back into scratch buffer (preserves allocation)
        self.scratch_draw_tiles = draw_tiles;

        // Interaction overlay: route polyline, markers, position puck —
        // always on top of tiles and labels.
        if !self.overlay.is_empty() {
            let camera = OverlayCamera {
                world_size,
                offset: map_offset,
                rect,
                meters_per_px: {
                    let (_, lat) = normalized_to_lon_lat(self.center_norm);
                    40_075_016.686 * lat.to_radians().cos() / world_size
                },
                rot: (rot_cos, rot_sin),
                rot_pivot,
                rotation_deg: self.rotation,
                tilt_cos: tilt_rad.cos(),
            };
            let mut overlay = std::mem::take(&mut self.overlay);
            draw_map_overlay(cx, &mut self.draw_overlay, &camera, &mut overlay);
            self.overlay = overlay;
        }

        let total_ms = perf_start.elapsed().as_secs_f64() * 1000.0;
        self.perf_frames += 1;
        self.perf_ms_total += total_ms;
        self.perf_ms_geo += geo_ms;
        self.perf_ms_labels += labels_ms;
        self.perf_ms_max = self.perf_ms_max.max(total_ms);
        if full_place {
            self.perf_label_full_places += 1;
        }
        if self.perf_frames >= 240 {
            use std::io::Write;
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("local/map_perf.log")
            {
                let frames = self.perf_frames as f64;
                let gap_avg = if self.perf_gap_count > 0 {
                    self.perf_gap_sum / self.perf_gap_count as f64
                } else {
                    0.0
                };
                let _ = writeln!(
                    file,
                    "frames:{} avg_ms:{:.2} geo_ms:{:.2} labels_ms:{:.2} max_ms:{:.2} gap_avg_ms:{:.2} gap_max_ms:{:.2} gaps>12ms:{}/{} full_places:{} glyphs:{} z:{:.2}",
                    self.perf_frames,
                    self.perf_ms_total / frames,
                    self.perf_ms_geo / frames,
                    self.perf_ms_labels / frames,
                    self.perf_ms_max,
                    gap_avg,
                    self.perf_ms_gap_max,
                    self.perf_gaps_over_12ms,
                    self.perf_gap_count,
                    self.perf_label_full_places,
                    self.label_perf.drawn_glyphs,
                    view_zoom,
                );
            }
            self.perf_frames = 0;
            self.perf_ms_total = 0.0;
            self.perf_ms_geo = 0.0;
            self.perf_ms_labels = 0.0;
            self.perf_ms_max = 0.0;
            self.perf_ms_gap_max = 0.0;
            self.perf_gap_sum = 0.0;
            self.perf_gap_count = 0;
            self.perf_gaps_over_12ms = 0;
            self.perf_label_full_places = 0;
        }

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
        let bucket = self.render_bucket();

        pool.execute_rev(tile_key, move |_tag| {
            match build_tile_buffers_from_body(tile_key, &body, &theme_style, bucket) {
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
        self.pending_ready_tiles.clear();
        self.tiles_generation = self.tiles_generation.wrapping_add(1);
        self.label_cache_valid = false;
    }

    fn apply_theme_palette(&mut self) {
        let (background, label) = {
            let style = self.active_style();
            (style.background, style.label)
        };
        self.draw_bg.color = background;
        self.draw_label.draw_super.color = label;
        self.draw_text.color = vec4(0.0, 0.0, 0.0, 1.0);
        // The background floor sits below the tilted map's negative depth
        // domain; everything drawn later (labels, panels, overlay) keeps
        // winning by ordinary call order.
        self.draw_bg.draw_depth = -50.0;
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

        let casing_geometry =
            if !buffers.casing_indices.is_empty() && !buffers.casing_vertices.is_empty() {
                let geometry = Geometry::new(cx);
                geometry.update(cx, buffers.casing_indices, buffers.casing_vertices);
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

        let icon_geometry = if !buffers.icon_indices.is_empty() && !buffers.icon_vertices.is_empty()
        {
            let geometry = Geometry::new(cx);
            geometry.update(cx, buffers.icon_indices, buffers.icon_vertices);
            Some(geometry)
        } else {
            None
        };

        // Cross-fade: keep the replaced generation's geometry under the new
        // one for TILE_FADE_SECONDS instead of popping.
        let fade = match self.tiles.remove(&tile_key) {
            Some(TileEntry {
                state:
                    TileLoadState::Ready {
                        fill_geometry: old_fill,
                        casing_geometry: old_casing,
                        stroke_geometry: old_stroke,
                        icon_geometry: old_icon,
                        ..
                    },
                bucket: old_bucket,
                ..
            }) => Some(TileFade {
                started: std::time::Instant::now(),
                bucket: old_bucket,
                fill_geometry: old_fill,
                casing_geometry: old_casing,
                stroke_geometry: old_stroke,
                icon_geometry: old_icon,
            }),
            _ => Some(TileFade {
                started: std::time::Instant::now(),
                bucket: buffers.render_zoom,
                fill_geometry: None,
                casing_geometry: None,
                stroke_geometry: None,
                icon_geometry: None,
            }),
        };
        cx.stop_timer(self.tile_fade_timer);
        self.tile_fade_timer = cx.start_timeout(0.016);

        self.tiles.insert(
            tile_key,
            TileEntry {
                state: TileLoadState::Ready {
                    fill_geometry,
                    casing_geometry,
                    stroke_geometry,
                    icon_geometry,
                    feature_count: buffers.feature_count,
                    labels: buffers.labels,
                },
                last_used: self.frame_counter,
                attempts: 0,
                bucket: buffers.render_zoom,
                fade,
            },
        );
        self.tiles_generation = self.tiles_generation.wrapping_add(1);
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
                        self.pending_ready_tiles
                            .retain(|(key, _)| *key != tile.tile_key);
                        self.pending_ready_tiles.push((tile.tile_key, tile.buffers));
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
                    self.pending_ready_tiles.retain(|(key, _)| *key != tile_key);
                    self.pending_ready_tiles.push((tile_key, buffers));
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
        // Drain at most two pending uploads per frame; a bucket-17+ tile can
        // carry tens of MB of vertex data, and creating/uploading a whole
        // 10-tile batch in one frame showed up as 200-550ms frame gaps.
        if !self.pending_ready_tiles.is_empty()
            && self.last_tile_upload_frame != self.frame_counter
        {
            self.last_tile_upload_frame = self.frame_counter;
            let upload_start = std::time::Instant::now();
            let count = self.pending_ready_tiles.len().min(2);
            let batch = self
                .pending_ready_tiles
                .drain(..count)
                .collect::<Vec<_>>();
            for (tile_key, buffers) in batch {
                self.insert_ready_tile(cx, tile_key, buffers);
            }
            let upload_ms = upload_start.elapsed().as_secs_f64() * 1000.0;
            if upload_ms > 4.0 {
                use std::io::Write;
                if let Ok(mut file) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open("local/map_perf.log")
                {
                    let _ = writeln!(file, "upload_ms:{:.2} tiles:{}", upload_ms, count);
                }
            }
            redraw = true;
        }
        if !self.pending_ready_tiles.is_empty() {
            redraw = true;
        }
        if redraw {
            self.update_status_text();
            self.redraw(cx);
        }
    }

    /// The active mbtiles source: the widget's `mbtiles_path` property when
    /// set, else the compiled-in default.
    fn active_mbtiles_path(&self) -> &str {
        if self.mbtiles_path.is_empty() {
            LOCAL_MBTILES_PATH
        } else {
            &self.mbtiles_path
        }
    }

    fn request_visible_tiles_from_local_source(&mut self, _cx: &mut Cx) {
        if !self.use_local_mbtiles {
            return;
        }

        let active_path = self.active_mbtiles_path().to_string();
        let mbtiles_path = Path::new(&active_path);
        if !mbtiles_path.is_file() && !self.local_source_missing_logged {
            log!("MapView: local mbtiles source missing at {} — serving disk tile cache only", active_path);
            self.local_source_missing_logged = true;
        }

        let bucket = self.render_bucket();
        // Watchdog: a worker job that dies (or a lost message) would leak its
        // key here forever and choke the 12-slot in-flight cap; time out and
        // retry, clearing any stuck Loading placeholder so it can re-request.
        let now = self.frame_counter;
        let timed_out: Vec<TileKey> = self
            .local_requested_tiles
            .iter()
            .filter(|(_, started)| now.saturating_sub(**started) > 600)
            .map(|(key, _)| *key)
            .collect();
        for key in timed_out {
            self.local_requested_tiles.remove(&key);
            if self
                .tiles
                .get(&key)
                .is_some_and(|entry| matches!(entry.state, TileLoadState::LoadingLocal))
            {
                self.tiles.remove(&key);
            }
        }
        // Mid-gesture the baked geometry scales geometrically (smooth); only
        // restyle stale buckets once the zoom has settled, or widths flicker
        // tile-batch by tile-batch under the gesture.
        let zoom_settling = self
            .last_zoom_change_time
            .is_some_and(|at| at.elapsed().as_secs_f64() < ZOOM_SETTLE_SECONDS);
        let mut missing = Vec::<TileKey>::new();
        for key in &self.visible_tiles {
            if self.local_requested_tiles.contains_key(key) || self.local_missing_tiles.contains(key) {
                continue;
            }
            if let Some(entry) = self.tiles.get(key) {
                match &entry.state {
                    // Stale zoom-bucket geometry stays drawable but gets rebuilt
                    TileLoadState::Ready { .. } if entry.bucket != bucket => {
                        if zoom_settling {
                            continue;
                        }
                    }
                    _ => continue,
                }
            }
            missing.push(*key);
        }
        if missing.is_empty() {
            return;
        }
        // Dispatch each tile as its own worker job so builds run in parallel
        // across the pool; keep enough in flight to cover a viewport restyle.
        let max_in_flight = 12usize.saturating_sub(self.local_requested_tiles.len());
        if missing.len() > max_in_flight {
            missing.truncate(max_in_flight);
        }
        if missing.is_empty() {
            return;
        }

        for key in &missing {
            self.local_requested_tiles.insert(*key, self.frame_counter);
            let keep_stale = self
                .tiles
                .get(key)
                .is_some_and(|entry| matches!(entry.state, TileLoadState::Ready { .. }));
            if !keep_stale {
                self.tiles.insert(
                    *key,
                    TileEntry {
                        state: TileLoadState::LoadingLocal,
                        last_used: self.frame_counter,
                        attempts: 0,
                        bucket,
                        fade: None,
                    },
                );
            }
        }

        let pool = self.tile_thread_pool.as_ref().unwrap();
        let style_epoch = self.style_epoch;
        for key in missing {
            let sender = self.tile_worker_rx.sender();
            let requested = vec![key];
            let mbtiles_path = active_path.clone();
            let detail_path = self.detail_mbtiles_path.clone();
            let overlay_paths: Vec<String> = self
                .overlay_mbtiles_paths
                .split(';')
                .filter(|p| !p.trim().is_empty())
                .map(|p| p.trim().to_string())
                .collect();
            // Extruded buildings only bake while the camera is tilted; flat
            // mode keeps the classic 2D building style with outlines.
            let buildings_3d = self.buildings_3d && self.tilt > 0.0;
            let theme_style = self.active_style().clone();
            pool.execute_rev(key, move |_tag| {
                let detail_path = (!detail_path.is_empty()).then_some(detail_path);
                let result = load_local_tile_batch(
                    Path::new(&mbtiles_path),
                    detail_path.as_deref().map(Path::new),
                    &overlay_paths,
                    &requested,
                    &theme_style,
                    bucket,
                    buildings_3d,
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
    }

    fn mark_tile_failed(&mut self, tile_key: TileKey, reason: &str) {
        let attempts = self
            .tiles
            .get(&tile_key)
            .map_or(1, |entry| entry.attempts.saturating_add(1));
        let retry_delay = retry_delay_frames(attempts);
        let retry_after = self.frame_counter.saturating_add(retry_delay);
        let bucket = self.render_bucket();
        self.tiles.insert(
            tile_key,
            TileEntry {
                state: TileLoadState::Failed { retry_after },
                last_used: self.frame_counter,
                attempts,
                bucket,
                fade: None,
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
        // Anchor into world-aligned space (undo rotation + tilt).
        let anchor_rel = self.screen_delta_to_world(anchor_abs - rect_center);
        let old_center_world = self.center_norm * old_world_size;
        let anchor_world = old_center_world + anchor_rel;
        let anchor_norm = anchor_world / old_world_size;
        let new_center_world = anchor_norm * new_world_size - anchor_rel;

        self.zoom = new_zoom;
        self.center_norm = new_center_world / new_world_size;
        self.wrap_and_clamp_center();
        self.last_zoom_change_frame = self.frame_counter;
        self.last_zoom_change_time = Some(std::time::Instant::now());
        self.pending_viewport_changed = true;
        // The paint beat idles when input stops; without a timer wake the
        // settle window would never elapse and stale-bucket restyles only
        // fired once the user wiggled the map again.
        cx.stop_timer(self.zoom_settle_timer);
        self.zoom_settle_timer = cx.start_timeout(0.15);
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
        // Read the archive's declared zoom range BEFORE computing visible
        // tile keys — request_zoom_level clamps to it, and reading it after
        // meant the very first frame requested impossible zoom levels.
        if self.use_local_mbtiles {
            let active_path = self.active_mbtiles_path().to_string();
            self.ensure_local_zoom_range(&active_path, Path::new(&active_path));
        }
        for entry in self.tiles.values_mut() {
            if entry
                .fade
                .as_ref()
                .is_some_and(|fade| fade.started.elapsed().as_secs_f64() > TILE_FADE_SECONDS)
            {
                entry.fade = None;
            }
        }
        self.visible_tiles = self.visible_tile_keys(rect);
        let target_zoom = self.request_zoom_level();
        // Keep frames coming briefly after a zoom change so the deferred
        // bucket restyle actually fires once the gesture settles.
        if self
            .last_zoom_change_time
            .is_some_and(|at| at.elapsed().as_secs_f64() < ZOOM_SETTLE_SECONDS + 0.05)
        {
            self.redraw(cx);
        }

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

        // Tiles at high buckets carry tens of MB of GPU buffers each; keeping
        // hundreds resident causes GPU memory pressure (frame-gap stutter).
        if self.tiles.len() > 48 {
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
                frame_counter.saturating_sub(entry.last_used) <= 120
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
        // Screen pixels cover 2^(view-zoom) request-zoom world pixels when
        // overzoomed; without this the viewport requests up to 64x too many
        // source tiles at view z17.
        let overzoom = 2.0_f64.powf(self.view_zoom() - zoom as f64).max(1.0);
        // Under heading-up rotation the viewport covers the rotated AABB in
        // world space.
        let (rot_cos, rot_sin) = self.screen_rotation();
        let half_w = rect.size.x * 0.5;
        // Tilt compression means the screen shows more world vertically.
        let half_h = rect.size.y * 0.5 / self.tilt_cos().max(1e-3);
        let half_size = dvec2(
            (half_w * rot_cos.abs() + half_h * rot_sin.abs()) / overzoom,
            (half_w * rot_sin.abs() + half_h * rot_cos.abs()) / overzoom,
        );
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

    /// A ready tile whose cross-fade started from no previous geometry —
    /// i.e. it is fading in over whatever was on screen before, not over an
    /// older restyle of itself.
    fn tile_fading_from_empty(&self, key: TileKey) -> bool {
        self.tiles.get(&key).is_some_and(|entry| {
            entry.fade.as_ref().is_some_and(|fade| {
                fade.fill_geometry.is_none()
                    && fade.casing_geometry.is_none()
                    && fade.stroke_geometry.is_none()
                    && fade.icon_geometry.is_none()
            })
        })
    }

    fn fill_draw_tile_keys(&mut self) {
        self.scratch_draw_tiles.clear();
        self.scratch_draw_seen.clear();

        for i in 0..self.visible_tiles.len() {
            let key = self.visible_tiles[i];
            if self.tile_is_ready(key) {
                // While this tile fades in from empty (fresh zoom level),
                // keep the previous zoom level's imagery painted beneath it
                // so the transition cross-fades instead of flashing the
                // background: prefer the ready ancestor, else descendants.
                if self.tile_fading_from_empty(key) {
                    if let Some(under) = self.find_ready_ancestor(key) {
                        if self.scratch_draw_seen.insert(under) {
                            self.scratch_draw_tiles.push(under);
                        }
                    } else {
                        self.fill_ready_descendants(key);
                        for j in 0..self.scratch_descendant_tiles.len() {
                            let under = self.scratch_descendant_tiles[j];
                            if !self.tile_fading_from_empty(under)
                                && self.scratch_draw_seen.insert(under)
                            {
                                self.scratch_draw_tiles.push(under);
                            }
                        }
                    }
                }
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
                let bucket = self.render_bucket();
                self.tiles.insert(
                    tile_key,
                    TileEntry {
                        state: TileLoadState::LoadingLocal,
                        last_used: self.frame_counter,
                        attempts: 0,
                        bucket,
                        fade: None,
                    },
                );
                pool.execute_rev(tile_key, move |_tag| {
                    match build_tile_buffers_from_body(tile_key, &cached_body, &theme_style, bucket)
                    {
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
        let bucket = self.render_bucket();
        self.tiles.insert(
            tile_key,
            TileEntry {
                state: TileLoadState::LoadingNetwork,
                last_used: self.frame_counter,
                attempts,
                bucket,
                fade: None,
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
        map_offset: Vec2d,
        rect: Rect,
    ) -> bool {
        // Pan-only frames: redraw the cached placement shifted by the pan
        // delta instead of re-scanning/re-shaping/re-colliding every label.
        let pan_delta = map_offset - self.label_cache_offset;
        let pan_dist = pan_delta.x.abs().max(pan_delta.y.abs());
        let rot_delta = self.rotation - self.label_cache_rotation;
        let cache_strict = self.label_cache_valid
            && self.label_cache_zoom == view_zoom
            && rot_delta == 0.0
            && self.label_cache_tilt == self.tilt
            && self.label_cache_generation == self.tiles_generation
            && self.label_cache_tiles.as_slice() == draw_tiles
            && pan_dist < LABEL_REPLACE_PAN_PX;
        // Softly-stale cache is still fine to show briefly; rate-limit the
        // expensive full re-place. This covers active zooming too — labels
        // stay pinned in screen space for up to ~125ms during the gesture
        // (pinch behavior a la Google Maps) instead of re-placing every
        // frame, which was 5-20ms/frame at label-dense zooms. Small
        // rotation deltas reuse the cache RIGIDLY rotated about the pivot —
        // that's what keeps labels from wiggling during heading-up nav —
        // but only at identical zoom (rotation+zoom compose non-affinely
        // with the cached-screen transform below).
        let cache_soft = self.label_cache_valid
            && (rot_delta == 0.0
                || (rot_delta.abs() <= 15.0 && self.label_cache_zoom == view_zoom))
            && self.label_cache_tilt == self.tilt
            && (self.label_cache_zoom - view_zoom).abs() < 0.5
            && self
                .last_full_place_time
                .is_some_and(|at| at.elapsed().as_secs_f64() < LABEL_REPLACE_MIN_SECONDS);
        if cache_strict || cache_soft {
            // Screen positions transform affinely under zoom-about-cursor:
            // s_new = s_old * k + R·(off_new - off_old * k) with the
            // heading-up rotation R applied about the view pivot. A plain
            // offset during zoom flung cached labels thousands of px away.
            let k = 2.0_f64.powf(view_zoom - self.label_cache_zoom);
            let raw_shift = map_offset - self.label_cache_offset * k;
            let camera_vec = |v: Vec2d| {
                let r = self.rotate_screen_vec(v);
                dvec2(r.x, r.y * self.tilt_cos())
            };
            let mut shift = camera_vec(raw_shift);
            if k != 1.0 {
                let pivot = rect.pos + rect.size * 0.5;
                shift += (pivot - camera_vec(pivot)) * (1.0 - k);
            }
            // Screen-space delta rotation about the view pivot (phi = -rotation).
            let rot_rad = (-rot_delta).to_radians() as f32;
            let pivot = rect.pos + rect.size * 0.5;
            self.draw_label_plans_scaled(
                cx,
                k as f32,
                Vec2f {
                    x: shift.x as f32,
                    y: shift.y as f32,
                },
                rot_rad,
                Vec2f {
                    x: pivot.x as f32,
                    y: pivot.y as f32,
                },
            );
            return false;
        }
        self.last_full_place_time = Some(std::time::Instant::now());

        let mut label_perf = LabelPerfStats::default();
        self.collect_label_candidates(draw_tiles, view_zoom, map_offset, rect, &mut label_perf);
        if self.scratch_candidates.is_empty() {
            self.path_glyphs.clear();
            self.scratch_accepted_plans.clear();
            self.store_label_cache(draw_tiles, view_zoom, map_offset);
            self.label_perf = label_perf;
            return true;
        }
        self.scratch_candidates
            .sort_unstable_by(|a, b| {
                b.score
                    .total_cmp(&a.score)
                    .then_with(|| a.name_key.cmp(&b.name_key))
            });
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

        // During gestures the budget keeps re-places to ~a frame; at rest run
        // a full pass, otherwise the tail (house numbers) would never place —
        // each pass restarts from the same highest-scored candidates.
        let at_rest = pan_dist < 1.0
            && (self.label_cache_zoom - view_zoom).abs() < 1e-9
            && self
                .last_zoom_change_time
                .is_none_or(|at| at.elapsed().as_secs_f64() > 0.25);
        let place_budget_ms = if at_rest { 40.0 } else { LABEL_PLACE_BUDGET_MS };
        let place_start = std::time::Instant::now();
        for candidate_index in 0..self.scratch_candidates.len() {
            if place_start.elapsed().as_secs_f64() * 1000.0 > place_budget_ms {
                label_perf.rejected_budget +=
                    label_perf.candidates_kept.saturating_sub(candidate_index);
                break;
            }
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
            self.scratch_accepted_hashes
                .push(stable_label_key(&candidate.name_key, &candidate.road_kind));
            self.scratch_accepted_plans.push((
                score,
                placement.glyph_start,
                placement.glyph_end,
                candidate.color_class,
            ));
        }

        self.prev_label_keys.clear();
        self.prev_label_keys
            .extend(self.scratch_accepted_hashes.drain(..));

        self.scratch_accepted_plans
            .sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        self.draw_label_plans(cx, Vec2f { x: 0.0, y: 0.0 });
        self.store_label_cache(draw_tiles, view_zoom, map_offset);
        // budget-truncated passes need another wake to place the tail
        self.needs_label_followup = label_perf.rejected_budget > 0;
        self.label_perf = label_perf;
        true
    }

    /// Draw the current accepted label plans (halo underdraw + colored text)
    /// as one glyph instance batch, optionally shifted by a screen offset
    /// (used to redraw the cached placement while panning).
    fn draw_label_plans(&mut self, cx: &mut Cx2d, extra_offset: Vec2f) {
        self.draw_label_plans_scaled(cx, 1.0, extra_offset, 0.0, Vec2f { x: 0.0, y: 0.0 });
    }

    fn draw_label_plans_scaled(
        &mut self,
        cx: &mut Cx2d,
        scale: f32,
        extra_offset: Vec2f,
        rot: f32,
        pivot: Vec2f,
    ) {
        // 4 diagonal offsets read as a solid halo at map label sizes and
        // halve the glyph volume vs an 8-direction ring
        const HALO_OFFSETS: [(f32, f32); 4] = [
            (-0.8, -0.8),
            (0.8, -0.8),
            (-0.8, 0.8),
            (0.8, 0.8),
        ];
        let dark_theme = self.dark_theme;
        let (label_color, halo_color) = {
            let style = self.active_style();
            (style.label, style.label_halo)
        };
        // Rigid delta-rotation of the cached placement about the pivot
        // (heading-up nav): transform a copy once, draw slices from it.
        let rotated: Vec<PathGlyphInstance> = if rot != 0.0 {
            let (c, s) = (rot.cos(), rot.sin());
            self.path_glyphs
                .iter()
                .map(|glyph| {
                    let mut glyph = glyph.clone();
                    let spin = |p: crate::makepad_draw::text::geom::Point<f32>| {
                        let dx = p.x - pivot.x;
                        let dy = p.y - pivot.y;
                        crate::makepad_draw::text::geom::Point::new(
                            pivot.x + dx * c - dy * s,
                            pivot.y + dx * s + dy * c,
                        )
                    };
                    glyph.glyph_origin = spin(glyph.glyph_origin);
                    glyph.rotation_origin = spin(glyph.rotation_origin);
                    glyph.angle += rot;
                    glyph
                })
                .collect()
        } else {
            Vec::new()
        };
        self.draw_label.begin_glyph_batch(cx);
        for i in 0..self.scratch_accepted_plans.len() {
            let (_, start, end, color_class) = self.scratch_accepted_plans[i];
            let glyphs = if rot != 0.0 {
                &rotated[start..end]
            } else {
                &self.path_glyphs[start..end]
            };
            self.draw_label.draw_super.color = halo_color;
            for offset in HALO_OFFSETS {
                self.draw_label.draw_path_glyphs_scaled(
                    cx,
                    glyphs,
                    scale,
                    Vec2f {
                        x: offset.0 + extra_offset.x,
                        y: offset.1 + extra_offset.y,
                    },
                );
            }
            self.draw_label.draw_super.color =
                label_class_color(color_class, label_color, dark_theme);
            self.draw_label.draw_path_glyphs_scaled(cx, glyphs, scale, extra_offset);
        }
        self.draw_label.end_glyph_batch(cx);
    }

    fn store_label_cache(&mut self, draw_tiles: &[TileKey], view_zoom: f64, map_offset: Vec2d) {
        self.label_cache_valid = true;
        self.label_cache_offset = map_offset;
        self.label_cache_zoom = view_zoom;
        self.label_cache_rotation = self.rotation;
        self.label_cache_tilt = self.tilt;
        self.label_cache_tiles.clear();
        self.label_cache_tiles.extend_from_slice(draw_tiles);
        self.label_cache_generation = self.tiles_generation;
    }

    fn collect_label_candidates(
        &mut self,
        draw_tiles: &[TileKey],
        view_zoom: f64,
        map_offset: Vec2d,
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

        let rot = self.screen_rotation();
        let rot_pivot = rect.pos + rect.size * 0.5;
        let tilt_cos = self.tilt_cos();
        let rotated = rot != (1.0, 0.0) || tilt_cos != 1.0;

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
            let scale64 = 2.0_f64.powf(view_zoom - key.z as f64);
            let scale = scale64 as f32;
            // Label paths are tile-local; add this tile's screen offset.
            let tile_offset = map_offset
                + dvec2(
                    key.x as f64 * TILE_SIZE * scale64,
                    key.y as f64 * TILE_SIZE * scale64,
                );
            let zoom_delta = (view_zoom - key.z as f64).abs();

            for label in labels {
                label_perf.labels_scanned += 1;
                let Some(source_rank) = label_source_rank(&label.source_layer) else {
                    continue;
                };
                let is_address = label.source_layer == "addresses";
                let is_poi = label.source_layer == "pois";
                // carto placenames zoom gates by settlement kind.
                let place = label.road_kind.strip_prefix("place:").map(|rest| {
                    let (kind, population) = rest.split_once(':').unwrap_or((rest, "0"));
                    (kind, population.parse::<u64>().unwrap_or(0))
                });
                if let Some((kind, _)) = place {
                    let min_zoom = match kind {
                        "city" => 4.0,
                        "town" => 7.0,
                        "village" | "suburb" => 11.5,
                        _ => 13.5,
                    };
                    let max_zoom = match kind {
                        "city" => 15.5,
                        "town" => 16.5,
                        _ => 17.0,
                    };
                    if view_zoom < min_zoom || view_zoom > max_zoom {
                        continue;
                    }
                }
                if is_address && view_zoom < ADDRESS_LABEL_MIN_ZOOM {
                    continue;
                }
                if is_poi && view_zoom < POI_LABEL_MIN_ZOOM {
                    continue;
                }
                // Cheap precomputed-bbox viewport reject before any path work;
                // most of an overzoomed tile's labels are far offscreen. The
                // bbox is world-aligned, so under rotation widen the margin
                // to the extra reach a rotated viewport corner can have.
                let bbox = label.bbox;
                let rot_margin = LABEL_VIEW_MARGIN
                    + if rotated {
                        (rect.size.x + rect.size.y) * 0.25
                    } else {
                        0.0
                    };
                if (bbox.2 as f64 * scale64 + tile_offset.x) < rect.pos.x - rot_margin
                    || (bbox.3 as f64 * scale64 + tile_offset.y) < rect.pos.y - rot_margin
                    || (bbox.0 as f64 * scale64 + tile_offset.x)
                        > rect.pos.x + rect.size.x + rot_margin
                    || (bbox.1 as f64 * scale64 + tile_offset.y)
                        > rect.pos.y + rect.size.y + rot_margin
                {
                    continue;
                }
                // precomputed at tile build; no per-frame allocation
                let name_key = &label.name_key;
                if name_key.len() < if is_address { 1 } else { 2 } {
                    continue;
                }

                // Build screen_path into scratch buffer, then move it into candidate
                self.scratch_screen_path.clear();
                build_screen_polyline_into(
                    &label.path_points,
                    scale,
                    tile_offset,
                    rot,
                    tilt_cos,
                    rot_pivot,
                    &mut self.scratch_screen_path,
                );
                // Point labels (addresses, POI names) stay upright: keep the
                // rotated anchor but restore a horizontal baseline.
                if rotated && (is_address || is_poi) && self.scratch_screen_path.len() == 2 {
                    let a = self.scratch_screen_path[0];
                    let b = self.scratch_screen_path[1];
                    let mid = (a + b) * 0.5;
                    let half = (b - a).length() * 0.5;
                    self.scratch_screen_path[0] = dvec2(mid.x - half, mid.y);
                    self.scratch_screen_path[1] = dvec2(mid.x + half, mid.y);
                }
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

                let repeat_distance = if is_address {
                    20.0
                } else {
                    repeat_distance_for_label(label.priority, source_rank)
                };
                // Use a fixed font_scale per tile zoom level so that labels
                // don't shift along the path during continuous zoom.
                // Grow street text with zoom the way carto does (~9px z14 -> ~12px z17).
                let mut font_scale =
                    0.92_f32 * (1.0 + 0.14 * (view_zoom - 14.0).clamp(0.0, 3.0) as f32);
                font_scale *= match label.priority {
                    1 => 1.08,
                    2 => 1.0,
                    _ => 0.92,
                };
                if is_address {
                    font_scale = 0.60;
                } else if is_poi {
                    font_scale = 0.72;
                } else if let Some((kind, population)) = place {
                    // Kind sets the class, population separates Amsterdam
                    // from Purmerend within it.
                    font_scale = match kind {
                        "city" => match population {
                            p if p >= 500_000 => 1.65,
                            p if p >= 150_000 => 1.4,
                            _ => 1.2,
                        },
                        "town" => 1.05,
                        "village" | "suburb" => 0.95,
                        _ => 0.88,
                    };
                }
                // quantize so the shaped-run cache hits during continuous zoom
                font_scale = (font_scale * 32.0).round() / 32.0;

                // Point-anchored area labels (parks, squares, zoo
                // enclosures) have a ~zero-length path; without a length
                // credit every street name outscores them in dense
                // viewports and they never place.
                let effective_length = if label.path_points.len() <= 2 {
                    path_length.max(420.0)
                } else {
                    path_length
                };
                let mut score = source_rank as f64 * 1000.0
                    + (4_u8.saturating_sub(label.priority) as f64) * 120.0
                    + (220.0 - zoom_delta * 65.0)
                    + effective_length.min(640.0) * 0.35;
                if let Some((_, population)) = place {
                    // log-population tiebreak inside a settlement tier.
                    score += (population.max(1) as f64).log10() * 15.0;
                }
                // Hysteresis: prefer labels that were visible last frame so
                // panning doesn't flicker between competing candidates.
                if self
                    .prev_label_keys
                    .contains(&stable_label_key(name_key, &label.road_kind))
                {
                    score += 350.0;
                }

                // Reuse existing candidate slot or push a new one
                if write_idx < self.scratch_candidates.len() {
                    let c = &mut self.scratch_candidates[write_idx];
                    c.text.push_str(&label.text);
                    c.name_key.push_str(name_key);
                    c.road_kind.push_str(&label.road_kind);
                    c.color_class = label.color_class;
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
                        name_key: name_key.clone(),
                        road_kind: label.road_kind.clone(),
                        color_class: label.color_class,
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

        // Shaping dominates placement cost; cache runs by (text, font_scale).
        let run_key = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            candidate.text.hash(&mut hasher);
            (
                hasher.finish(),
                candidate.text.len() as u32,
                candidate.font_scale.to_bits(),
            )
        };
        if !self.shaped_runs.contains_key(&run_key) {
            if self.shaped_runs.len() > 4096 {
                self.shaped_runs.clear();
            }
            self.draw_label.draw_super.font_scale = candidate.font_scale;
            let shaped = self
                .draw_label
                .draw_super
                .prepare_single_line_run(cx, candidate.text.as_str())
                .filter(|run| !run.glyphs.is_empty());
            self.shaped_runs.insert(run_key, shaped);
        }
        let run = match self.shaped_runs.get(&run_key) {
            Some(Some(run)) => run.clone(),
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
        // Reading direction from the chord across the whole text span: a
        // single mid-point tangent can point 180 degrees off on zigzag
        // segments (rail-yard paths), flipping the label upside down.
        let span_a = sample_polyline_point_at_distance(&smooth_a, &cum, start_distance);
        let span_b = sample_polyline_point_at_distance(
            &smooth_a,
            &cum,
            start_distance + text_width as f64,
        );
        let reverse = match (span_a, span_b) {
            (Some(a), Some(b)) if (b.x - a.x).abs() + (b.y - a.y).abs() > 6.0 => {
                choose_label_reverse(((b.y - a.y) as f32).atan2((b.x - a.x) as f32))
            }
            _ => choose_label_reverse(mid_tangent_angle),
        };
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

    /// (cos, sin) of the screen rotation φ = -rotation — the transform that
    /// makes the `rotation` bearing point up. Identity when north-up.
    fn screen_rotation(&self) -> (f64, f64) {
        if self.rotation == 0.0 {
            return (1.0, 0.0);
        }
        let phi = -self.rotation.to_radians();
        (phi.cos(), phi.sin())
    }

    /// Rotate a screen vector from unrotated (world-aligned) space into
    /// rotated screen space.
    fn rotate_screen_vec(&self, v: Vec2d) -> Vec2d {
        let (cos, sin) = self.screen_rotation();
        dvec2(v.x * cos - v.y * sin, v.x * sin + v.y * cos)
    }

    /// Inverse: rotated screen vector back into world-aligned screen space.
    fn unrotate_screen_vec(&self, v: Vec2d) -> Vec2d {
        let (cos, sin) = self.screen_rotation();
        dvec2(v.x * cos + v.y * sin, -v.x * sin + v.y * cos)
    }

    fn tilt_cos(&self) -> f64 {
        self.tilt.clamp(0.0, 65.0).to_radians().cos()
    }

    /// Screen-space vector (relative to the view pivot) back into
    /// world-aligned space: undo the tilt compression, then the rotation.
    fn screen_delta_to_world(&self, v: Vec2d) -> Vec2d {
        let tilt_cos = self.tilt_cos().max(1e-3);
        self.unrotate_screen_vec(dvec2(v.x, v.y / tilt_cos))
    }

    fn request_zoom_level(&self) -> u32 {
        let mut zoom = self.view_zoom().round() as u32;
        if self.use_local_mbtiles {
            // Honor the archive's declared zoom range: a single-zoom detail
            // archive (minzoom=maxzoom=14) must never be asked for z13/z12 —
            // those rows cannot exist and only produce missing-tile spam.
            let (min_zoom, max_zoom) = self
                .local_source_zoom_range
                .unwrap_or((LOCAL_MBTILES_MIN_ZOOM, LOCAL_MBTILES_MAX_ZOOM));
            zoom = zoom.clamp(min_zoom, max_zoom);
        }
        zoom
    }

    /// Read the active archive's declared minzoom/maxzoom once per path.
    /// Opening is cheap (metadata B-tree only); absent/invalid metadata
    /// falls back to the compiled-in range.
    fn ensure_local_zoom_range(&mut self, active_path: &str, mbtiles_path: &Path) {
        let file_exists = mbtiles_path.is_file();
        let same_path = self
            .local_source_zoom_range_path
            .as_deref()
            .is_some_and(|p| p == active_path);
        // Re-attempt only on a path change, or when the archive appears after
        // a missing-file attempt (e.g. a conversion finishing mid-session).
        if same_path && (self.local_source_zoom_range_checked || !file_exists) {
            return;
        }
        self.local_source_zoom_range_path = Some(active_path.to_string());
        self.local_source_zoom_range = None;
        self.local_source_zoom_range_checked = file_exists;
        if !file_exists {
            return;
        }
        let range = MbtilesReader::open(mbtiles_path)
            .ok()
            .and_then(|mut reader| reader.get_metadata().ok())
            .and_then(|metadata| {
                let min = metadata.get("minzoom")?.trim().parse::<u32>().ok()?;
                let max = metadata.get("maxzoom")?.trim().parse::<u32>().ok()?;
                (min <= max).then_some((min, max))
            });
        if let Some((min, max)) = range {
            self.local_source_zoom_range = Some((min, max));
            if (min, max) != (LOCAL_MBTILES_MIN_ZOOM, LOCAL_MBTILES_MAX_ZOOM) {
                log!(
                    "MapView: {} declares zoom range z{}-z{}; clamping tile requests",
                    active_path,
                    min,
                    max
                );
            }
        }
    }

    /// View-zoom bucket the tile styling (widths, AA, outlines) is built for.
    /// Beyond the source max zoom the same z14 tiles are re-styled per bucket.
    fn render_bucket(&self) -> u32 {
        self.view_zoom().round() as u32
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

// --- Camera + overlay public API (the M0 interaction surface) ---

impl MapView {
    fn overlay_camera(&self) -> OverlayCamera {
        let world_size = tile_world_size_zoom(self.view_zoom());
        let center_world = self.center_norm * world_size;
        let rect = self.view_rect;
        let offset = dvec2(
            rect.pos.x + rect.size.x * 0.5 - center_world.x,
            rect.pos.y + rect.size.y * 0.5 - center_world.y,
        );
        let (_, lat) = normalized_to_lon_lat(self.center_norm);
        OverlayCamera {
            world_size,
            offset,
            rect,
            meters_per_px: 40_075_016.686 * lat.to_radians().cos() / world_size,
            rot: self.screen_rotation(),
            rot_pivot: rect.pos + rect.size * 0.5,
            rotation_deg: self.rotation,
            tilt_cos: self.tilt_cos(),
        }
    }

    fn sync_camera_fields(&mut self) {
        let (lon, lat) = normalized_to_lon_lat(self.center_norm);
        self.center_lon = lon;
        self.center_lat = lat;
    }

    fn emit_viewport_changed(&mut self, cx: &mut Cx) {
        cx.widget_action(
            self.uid,
            MapViewAction::ViewportChanged {
                lon: self.center_lon,
                lat: self.center_lat,
                zoom: self.view_zoom(),
            },
        );
    }

    pub fn screen_to_lon_lat(&self, abs: Vec2d) -> (f64, f64) {
        let camera = self.overlay_camera();
        let pivot = camera.rot_pivot;
        let unrotated = self.screen_delta_to_world(abs - pivot) + pivot;
        let norm = (unrotated - camera.offset) / camera.world_size;
        normalized_to_lon_lat(norm)
    }

    pub fn lon_lat_to_screen(&self, lon: f64, lat: f64) -> Vec2d {
        let camera = self.overlay_camera();
        camera.norm_to_screen(lon_lat_to_normalized(lon, lat))
    }

    pub fn center(&self) -> (f64, f64) {
        normalized_to_lon_lat(self.center_norm)
    }

    pub fn map_zoom(&self) -> f64 {
        self.view_zoom()
    }

    pub fn set_center(&mut self, cx: &mut Cx, lon: f64, lat: f64) {
        self.fly = None;
        self.center_norm = lon_lat_to_normalized(lon, lat);
        self.wrap_and_clamp_center();
        self.sync_camera_fields();
        self.redraw(cx);
    }

    /// Heading-up camera: the given bearing (degrees, 0 = north) points up.
    pub fn set_rotation(&mut self, cx: &mut Cx, rotation_deg: f64) {
        let rotation = rotation_deg.rem_euclid(360.0);
        if (rotation - self.rotation).abs() < 1e-9 {
            return;
        }
        self.rotation = rotation;
        self.redraw(cx);
    }

    pub fn rotation(&self) -> f64 {
        self.rotation
    }

    /// Axonometric camera tilt (degrees, 0 = top-down, clamped to 65).
    /// Crossing between flat and tilted rebakes tiles: flat mode uses the
    /// true 2D building style (base fills + outlines), tilted mode the
    /// extruded detail buildings.
    pub fn set_tilt(&mut self, cx: &mut Cx, tilt_deg: f64) {
        let tilt = tilt_deg.clamp(0.0, 65.0);
        if (tilt - self.tilt).abs() < 1e-9 {
            return;
        }
        let was_3d = self.tilt > 0.0;
        self.tilt = tilt;
        if was_3d != (self.tilt > 0.0) {
            self.restyle_tiles_keep_stale(cx);
        }
        self.redraw(cx);
    }

    /// Rebuild every resident tile under the current style/mode while its
    /// previous geometry stays on screen (bucket sentinel → the normal
    /// stale-bucket restyle path picks it up and cross-fades).
    /// Swap the active geodata overlays; stale tiles keep rendering while
    /// rebuilt ones stream in with the new layer set.
    pub fn set_overlay_paths(&mut self, cx: &mut Cx, paths: &str) {
        if self.overlay_mbtiles_paths == paths {
            return;
        }
        self.overlay_mbtiles_paths = paths.to_string();
        self.restyle_tiles_keep_stale(cx);
    }

    fn restyle_tiles_keep_stale(&mut self, cx: &mut Cx) {
        self.style_epoch = self.style_epoch.wrapping_add(1);
        if self.style_epoch == 0 {
            self.style_epoch = 1;
        }
        for entry in self.tiles.values_mut() {
            if matches!(entry.state, TileLoadState::Ready { .. }) {
                entry.bucket = u32::MAX;
            }
        }
        self.local_requested_tiles.clear();
        self.pending_ready_tiles.clear();
        self.label_cache_valid = false;
        self.redraw(cx);
    }

    pub fn tilt(&self) -> f64 {
        self.tilt
    }

    pub fn set_map_zoom(&mut self, cx: &mut Cx, zoom: f64) {
        let min_zoom = self.min_zoom.max(0.0);
        let max_zoom = self.max_zoom.max(min_zoom);
        self.fly = None;
        self.zoom = zoom.clamp(min_zoom, max_zoom);
        self.last_zoom_change_frame = self.frame_counter;
        self.last_zoom_change_time = Some(std::time::Instant::now());
        cx.stop_timer(self.zoom_settle_timer);
        self.zoom_settle_timer = cx.start_timeout(0.15);
        self.redraw(cx);
    }

    /// Animated camera flight; far targets get a zoom-out-then-in arc so
    /// tiles stay loadable mid-flight and the motion reads like every
    /// mapping app.
    pub fn fly_to(&mut self, cx: &mut Cx, lon: f64, lat: f64, zoom: f64) {
        let min_zoom = self.min_zoom.max(0.0);
        let max_zoom = self.max_zoom.max(min_zoom);
        let to_zoom = zoom.clamp(min_zoom, max_zoom);
        let from_zoom = self.view_zoom();
        let to_center = lon_lat_to_normalized(lon, lat);
        let dist_px = (to_center - self.center_norm).length() * tile_world_size_zoom(from_zoom);
        let viewport = self.view_rect.size.length().max(400.0);
        let arc = if dist_px > viewport * 0.5 {
            ((dist_px / viewport).log2() * 0.9).clamp(0.4, 4.5)
        } else {
            0.0
        };
        let duration = (0.55 + 0.22 * arc + (dist_px / 6000.0).min(0.6)).min(2.4);
        self.fly = Some(FlyTo {
            started: std::time::Instant::now(),
            duration,
            from_center: self.center_norm,
            to_center,
            from_zoom,
            to_zoom,
            arc,
        });
        cx.stop_timer(self.fly_timer);
        self.fly_timer = cx.start_timeout(0.016);
        self.redraw(cx);
    }

    fn tick_fly(&mut self, cx: &mut Cx) {
        let Some(fly) = self.fly else {
            return;
        };
        let min_zoom = self.min_zoom.max(0.0);
        let max_zoom = self.max_zoom.max(min_zoom);
        let t = (fly.started.elapsed().as_secs_f64() / fly.duration).clamp(0.0, 1.0);
        let e = t * t * (3.0 - 2.0 * t);
        self.center_norm = fly.from_center + (fly.to_center - fly.from_center) * e;
        let zoom = fly.from_zoom + (fly.to_zoom - fly.from_zoom) * e
            - fly.arc * (std::f64::consts::PI * e).sin();
        self.zoom = zoom.clamp(min_zoom, max_zoom);
        self.wrap_and_clamp_center();
        self.last_zoom_change_frame = self.frame_counter;
        self.last_zoom_change_time = Some(std::time::Instant::now());
        if t >= 1.0 {
            self.fly = None;
            self.center_norm = fly.to_center;
            self.zoom = fly.to_zoom;
            self.wrap_and_clamp_center();
            self.sync_camera_fields();
            self.emit_viewport_changed(cx);
            cx.stop_timer(self.zoom_settle_timer);
            self.zoom_settle_timer = cx.start_timeout(0.15);
        } else {
            self.fly_timer = cx.start_timeout(0.016);
        }
        self.redraw(cx);
    }

    // --- Overlay content ---

    pub fn set_markers(&mut self, cx: &mut Cx, markers: Vec<MapMarker>) {
        self.overlay.markers = markers;
        self.redraw(cx);
    }

    /// Route polyline as (lon, lat) pairs; resets travel progress.
    pub fn set_route(&mut self, cx: &mut Cx, points: &[(f64, f64)]) {
        self.overlay.route = Some(MapRouteOverlay {
            points_norm: points
                .iter()
                .map(|&(lon, lat)| lon_lat_to_normalized(lon, lat))
                .collect(),
            traveled_index: 0,
        });
        self.redraw(cx);
    }

    pub fn clear_route(&mut self, cx: &mut Cx) {
        self.overlay.route = None;
        self.redraw(cx);
    }

    /// Points before `index` draw dimmed (the already-driven part).
    pub fn set_route_progress(&mut self, cx: &mut Cx, index: usize) {
        if let Some(route) = &mut self.overlay.route {
            if route.traveled_index != index {
                route.traveled_index = index;
                self.redraw(cx);
            }
        }
    }

    pub fn set_puck(&mut self, cx: &mut Cx, puck: Option<MapPuck>) {
        self.overlay.puck = puck;
        self.redraw(cx);
    }
}

impl MapViewRef {
    pub fn tapped(&self, actions: &Actions) -> Option<(f64, f64)> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let MapViewAction::Tapped { lon, lat, .. } = item.cast() {
                return Some((lon, lat));
            }
        }
        None
    }

    pub fn long_pressed(&self, actions: &Actions) -> Option<(f64, f64)> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let MapViewAction::LongPressed { lon, lat, .. } = item.cast() {
                return Some((lon, lat));
            }
        }
        None
    }

    pub fn viewport_changed(&self, actions: &Actions) -> Option<(f64, f64, f64)> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let MapViewAction::ViewportChanged { lon, lat, zoom } = item.cast() {
                return Some((lon, lat, zoom));
            }
        }
        None
    }

    pub fn marker_clicked(&self, actions: &Actions) -> Option<u64> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let MapViewAction::MarkerClicked { id } = item.cast() {
                return Some(id);
            }
        }
        None
    }

    pub fn center(&self) -> Option<(f64, f64)> {
        self.borrow().map(|inner| inner.center())
    }

    pub fn map_zoom(&self) -> Option<f64> {
        self.borrow().map(|inner| inner.map_zoom())
    }

    pub fn set_center(&self, cx: &mut Cx, lon: f64, lat: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_center(cx, lon, lat);
        }
    }

    pub fn set_map_zoom(&self, cx: &mut Cx, zoom: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_map_zoom(cx, zoom);
        }
    }

    pub fn set_overlay_paths(&self, cx: &mut Cx, paths: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_overlay_paths(cx, paths);
        }
    }

    pub fn set_rotation(&self, cx: &mut Cx, rotation_deg: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_rotation(cx, rotation_deg);
        }
    }

    pub fn rotation(&self) -> f64 {
        self.borrow().map(|inner| inner.rotation()).unwrap_or(0.0)
    }

    pub fn set_tilt(&self, cx: &mut Cx, tilt_deg: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_tilt(cx, tilt_deg);
        }
    }

    pub fn tilt(&self) -> f64 {
        self.borrow().map(|inner| inner.tilt()).unwrap_or(0.0)
    }

    pub fn fly_to(&self, cx: &mut Cx, lon: f64, lat: f64, zoom: f64) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.fly_to(cx, lon, lat, zoom);
        }
    }

    pub fn set_markers(&self, cx: &mut Cx, markers: Vec<MapMarker>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_markers(cx, markers);
        }
    }

    pub fn set_route(&self, cx: &mut Cx, points: &[(f64, f64)]) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_route(cx, points);
        }
    }

    pub fn clear_route(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.clear_route(cx);
        }
    }

    pub fn set_route_progress(&self, cx: &mut Cx, index: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_route_progress(cx, index);
        }
    }

    pub fn set_puck(&self, cx: &mut Cx, puck: Option<MapPuck>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_puck(cx, puck);
        }
    }
}

/// Viewport-independent identity for a label, used for frame-to-frame
/// placement hysteresis (road_kind embeds the tile-local position for points).
fn stable_label_key(name_key: &str, road_kind: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    name_key.hash(&mut hasher);
    road_kind.hash(&mut hasher);
    hasher.finish()
}

/// Carto-style label colors per POI class (orange food, purple shops,
/// brown culture, muted house numbers).
fn label_class_color(color_class: u8, default_color: Vec4f, dark_theme: bool) -> Vec4f {
    match (color_class, dark_theme) {
        (LABEL_CLASS_AMENITY, false) => Vec4f::from_u32(0xc77400ff),
        (LABEL_CLASS_SHOP, false) => Vec4f::from_u32(0xac39acff),
        (LABEL_CLASS_CULTURE, false) => Vec4f::from_u32(0x734a08ff),
        (LABEL_CLASS_MUTED, false) => Vec4f::from_u32(0x66768dff),
        (LABEL_CLASS_HEALTH, false) => Vec4f::from_u32(0xbf0000ff),
        (LABEL_CLASS_GREEN, false) => Vec4f::from_u32(0x267d3fff),
        (LABEL_CLASS_AMENITY, true) => Vec4f::from_u32(0xe09a4aff),
        (LABEL_CLASS_SHOP, true) => Vec4f::from_u32(0xcf7fcfff),
        (LABEL_CLASS_CULTURE, true) => Vec4f::from_u32(0xc9a36cff),
        (LABEL_CLASS_MUTED, true) => Vec4f::from_u32(0x8899aaff),
        (LABEL_CLASS_HEALTH, true) => Vec4f::from_u32(0xe06666ff),
        (LABEL_CLASS_GREEN, true) => Vec4f::from_u32(0x7fc98fff),
        (LABEL_CLASS_WATER, false) => Vec4f::from_u32(0x39688fff),
        (LABEL_CLASS_WATER, true) => Vec4f::from_u32(0x7fb2d9ff),
        _ => default_color,
    }
}
