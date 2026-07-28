use super::geometry::{is_road_polygon_layer, tag_is, tag_is_truthy};
use crate::makepad_draw::*;
use std::collections::HashMap;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.MapFillRule = #(MapFillRule::script_api(vm))
    mod.widgets.MapRoadRule = #(MapRoadRule::script_api(vm))
    mod.widgets.MapWaterwayRule = #(MapWaterwayRule::script_api(vm))
    mod.widgets.MapRailRule = #(MapRailRule::script_api(vm))
    mod.widgets.MapThemeStyle = #(MapThemeStyle::script_component(vm))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StrokePassStyle {
    pub color: u32,
    pub width: f32,
    pub shape_id: f32,
    /// Width-growth class for GPU stroke re-expansion: which zoom curve the
    /// baked width follows, so stale-bucket tiles can be corrected to the
    /// width the current view zoom calls for.
    pub expand_class: f32,
    /// Tilt-mode micro-depth (sort-rank scaled): separates overlapping
    /// strokes (rail over road) by more than the depth-buffer quantum while
    /// staying far below one ground pixel of view depth.
    pub depth_micro: f32,
}

/// Micro-depth per unit of sort rank (rank 710 rail → 0.0014).
pub const DEPTH_MICRO_PER_RANK: f32 = 4e-5;

/// Regular roads: widths follow `zoom_width_mult` directly.
pub const EXPAND_CLASS_ROAD: f32 = 0.0;
/// Thin uncased paths + rails: grow as `max(1, mult)^0.35`.
pub const EXPAND_CLASS_THIN: f32 = 1.0;
/// Waterway centerlines: shrink as `mult^1.6` below z14, linear above.
pub const EXPAND_CLASS_WATER: f32 = 2.0;
/// Constant screen-px strokes (building outlines).
pub const EXPAND_CLASS_CONST_PX: f32 = 3.0;

/// Per-class width correction for drawing geometry baked at `bucket` while
/// the camera sits at (fractional) `view_zoom`: multiplying the baked stroke
/// offsets by this factor yields the width a fresh restyle at `view_zoom`
/// would bake. x=road, y=thin, z=water, w=constant-px.
pub fn stroke_width_correction(bucket: u32, view_zoom: f64) -> [f32; 4] {
    let geom_scale = 2.0_f64.powf(view_zoom - bucket as f64);
    let s_view = zoom_width_mult_continuous(view_zoom);
    let s_bucket = zoom_width_mult(bucket) as f64;
    let thin = |s: f64| s.max(1.0).powf(0.35);
    let water = |s: f64| if s < 1.0 { s.powf(1.6) } else { s };
    [
        (s_view / (s_bucket * geom_scale)) as f32,
        (thin(s_view) / (thin(s_bucket) * geom_scale)) as f32,
        (water(s_view) / (water(s_bucket) * geom_scale)) as f32,
        (1.0 / geom_scale) as f32,
    ]
}

/// `zoom_width_mult` interpolated between integer buckets so corrections
/// stay smooth through a zoom gesture.
fn zoom_width_mult_continuous(view_zoom: f64) -> f64 {
    let floor = view_zoom.floor().max(0.0);
    let frac = (view_zoom - floor).clamp(0.0, 1.0);
    let a = zoom_width_mult(floor as u32) as f64;
    let b = zoom_width_mult(floor as u32 + 1) as f64;
    a + (b - a) * frac
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StrokeStyle {
    pub sort_rank: i16,
    pub casing: Option<StrokePassStyle>,
    pub center: StrokePassStyle,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct StrokeTemplate {
    sort_rank: i16,
    casing: Option<StrokePassStyle>,
    center: StrokePassStyle,
    /// Lowest render bucket this rule draws at (carto hides paths and small
    /// waterways well before roads).
    min_zoom: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CompiledMapTheme {
    pub background: Vec4f,
    pub status_text: Vec4f,
    pub label: Vec4f,
    pub label_halo: Vec4f,
    building_fill: Option<u32>,
    pub building_outline: Option<u32>,
    street_area_fill: Option<u32>,
    bridge_area_fill: Option<u32>,
    water_fill: Option<u32>,
    landuse_fills: HashMap<String, u32>,
    landuse_default: Option<u32>,
    leisure_fills: HashMap<String, u32>,
    leisure_default: Option<u32>,
    road_rules: HashMap<String, StrokeTemplate>,
    road_default: Option<StrokeTemplate>,
    waterway_rules: HashMap<String, StrokeTemplate>,
    waterway_default: Option<StrokeTemplate>,
    railway_rule: Option<StrokeTemplate>,
}

impl Default for CompiledMapTheme {
    fn default() -> Self {
        Self {
            background: Vec4f::from_u32(0xddd7ccff),
            status_text: Vec4f::from_u32(0xdee9f4ff),
            label: Vec4f::from_u32(0x000000ff),
            label_halo: Vec4f::from_u32(0xffffffff),
            building_fill: None,
            building_outline: None,
            street_area_fill: None,
            bridge_area_fill: None,
            water_fill: None,
            landuse_fills: HashMap::new(),
            landuse_default: None,
            leisure_fills: HashMap::new(),
            leisure_default: None,
            road_rules: HashMap::new(),
            road_default: None,
            waterway_rules: HashMap::new(),
            waterway_default: None,
            railway_rule: None,
        }
    }
}

#[derive(Script, ScriptHook, Clone, Default)]
pub struct MapFillRule {
    #[source]
    source: ScriptObjectRef,
    #[live]
    pub group: String,
    #[live]
    pub value: String,
    #[live]
    pub color: Vec4f,
}

#[derive(Script, ScriptHook, Clone, Default)]
pub struct MapRoadRule {
    #[source]
    source: ScriptObjectRef,
    #[live]
    pub kind: String,
    #[live]
    pub sort_rank: u32,
    #[live]
    pub casing_color: Vec4f,
    #[live]
    pub casing_width: f32,
    #[live]
    pub casing_shape_id: f32,
    #[live]
    pub center_color: Vec4f,
    #[live]
    pub center_width: f32,
    #[live]
    pub center_shape_id: f32,
    /// Hidden below this render zoom (0 = always visible).
    #[live]
    pub min_zoom: f32,
}

#[derive(Script, ScriptHook, Clone, Default)]
pub struct MapWaterwayRule {
    #[source]
    source: ScriptObjectRef,
    #[live]
    pub kind: String,
    #[live]
    pub sort_rank: u32,
    #[live]
    pub casing_color: Vec4f,
    #[live]
    pub casing_width: f32,
    #[live]
    pub casing_shape_id: f32,
    #[live]
    pub center_color: Vec4f,
    #[live]
    pub center_width: f32,
    #[live]
    pub center_shape_id: f32,
    /// Hidden below this render zoom (0 = always visible).
    #[live]
    pub min_zoom: f32,
}

#[derive(Script, ScriptHook, Clone, Default)]
pub struct MapRailRule {
    #[source]
    source: ScriptObjectRef,
    #[live]
    pub sort_rank: u32,
    #[live]
    pub casing_color: Vec4f,
    #[live]
    pub casing_width: f32,
    #[live]
    pub casing_shape_id: f32,
    #[live]
    pub center_color: Vec4f,
    #[live]
    pub center_width: f32,
    #[live]
    pub center_shape_id: f32,
}

#[derive(Script, Clone)]
pub struct MapThemeStyle {
    #[source]
    source: ScriptObjectRef,
    #[live]
    pub background: Vec4f,
    #[live]
    pub status_text: Vec4f,
    #[live]
    pub label: Vec4f,
    #[live(vec4(1.0, 1.0, 1.0, 1.0))]
    pub label_halo: Vec4f,
    #[rust]
    fill_rules: Vec<MapFillRule>,
    #[rust]
    road_rules: Vec<MapRoadRule>,
    #[rust]
    waterway_rules: Vec<MapWaterwayRule>,
    #[rust]
    railway_rule: Option<MapRailRule>,
}

impl Default for MapThemeStyle {
    fn default() -> Self {
        Self {
            source: Default::default(),
            background: Vec4f::from_u32(0xddd7ccff),
            status_text: Vec4f::from_u32(0xdee9f4ff),
            label: Vec4f::from_u32(0x000000ff),
            label_halo: Vec4f::from_u32(0xffffffff),
            fill_rules: Vec::new(),
            road_rules: Vec::new(),
            waterway_rules: Vec::new(),
            railway_rule: None,
        }
    }
}

impl ScriptHook for MapThemeStyle {
    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        value: ScriptValue,
    ) {
        self.fill_rules.clear();
        self.road_rules.clear();
        self.waterway_rules.clear();
        self.railway_rule = None;

        if let Some(obj) = value.as_object() {
            vm.vec_with(obj, |vm, vec| {
                for kv in vec {
                    let Some(obj) = kv.value.as_object() else {
                        continue;
                    };
                    if vm
                        .bx
                        .heap
                        .type_matches_id(obj, MapFillRule::script_type_id_static())
                    {
                        self.fill_rules
                            .push(MapFillRule::script_from_value(vm, kv.value));
                    } else if vm
                        .bx
                        .heap
                        .type_matches_id(obj, MapRoadRule::script_type_id_static())
                    {
                        self.road_rules
                            .push(MapRoadRule::script_from_value(vm, kv.value));
                    } else if vm
                        .bx
                        .heap
                        .type_matches_id(obj, MapWaterwayRule::script_type_id_static())
                    {
                        self.waterway_rules
                            .push(MapWaterwayRule::script_from_value(vm, kv.value));
                    } else if vm
                        .bx
                        .heap
                        .type_matches_id(obj, MapRailRule::script_type_id_static())
                    {
                        self.railway_rule = Some(MapRailRule::script_from_value(vm, kv.value));
                    }
                }
            });
        }
    }
}

impl MapThemeStyle {
    pub fn compile(&self) -> CompiledMapTheme {
        let mut compiled = CompiledMapTheme {
            background: self.background,
            status_text: self.status_text,
            label: self.label,
            label_halo: self.label_halo,
            ..CompiledMapTheme::default()
        };

        for rule in &self.fill_rules {
            let group = rule.group.trim().to_ascii_lowercase();
            if group.is_empty() {
                continue;
            }
            let value = rule.value.trim().to_ascii_lowercase();
            let color = vec4_to_rgb_hex(rule.color);

            match group.as_str() {
                "building" => compiled.building_fill = Some(color),
                "building_outline" => compiled.building_outline = Some(color),
                "street_area" => compiled.street_area_fill = Some(color),
                "bridge_area" => compiled.bridge_area_fill = Some(color),
                "water" => compiled.water_fill = Some(color),
                "landuse" => {
                    if is_default_key(value.as_str()) {
                        compiled.landuse_default = Some(color);
                    } else {
                        compiled.landuse_fills.insert(value, color);
                    }
                }
                "leisure" => {
                    if is_default_key(value.as_str()) {
                        compiled.leisure_default = Some(color);
                    } else {
                        compiled.leisure_fills.insert(value, color);
                    }
                }
                _ => {}
            }
        }

        for rule in &self.road_rules {
            let kind = rule.kind.trim().to_ascii_lowercase();
            let template = stroke_template_from_road_rule(rule);
            if is_default_key(kind.as_str()) {
                compiled.road_default = Some(template);
            } else {
                compiled.road_rules.insert(kind, template);
            }
        }

        for rule in &self.waterway_rules {
            let kind = rule.kind.trim().to_ascii_lowercase();
            let template = stroke_template_from_waterway_rule(rule);
            if is_default_key(kind.as_str()) {
                compiled.waterway_default = Some(template);
            } else {
                compiled.waterway_rules.insert(kind, template);
            }
        }

        if let Some(rule) = &self.railway_rule {
            compiled.railway_rule = Some(stroke_template_from_rail_rule(rule));
        }

        compiled
    }
}

fn stroke_template_from_road_rule(rule: &MapRoadRule) -> StrokeTemplate {
    StrokeTemplate {
        sort_rank: clamp_u32_to_i16(rule.sort_rank),
        casing: if rule.casing_width > 0.0 {
            Some(StrokePassStyle {
                color: vec4_to_rgb_hex(rule.casing_color),
                width: rule.casing_width,
                shape_id: rule.casing_shape_id,
                expand_class: EXPAND_CLASS_ROAD,
                depth_micro: rule.sort_rank as f32 * DEPTH_MICRO_PER_RANK,
            })
        } else {
            None
        },
        center: StrokePassStyle {
            color: vec4_to_rgb_hex(rule.center_color),
            width: rule.center_width,
            shape_id: rule.center_shape_id,
            expand_class: EXPAND_CLASS_ROAD,
            depth_micro: rule.sort_rank as f32 * DEPTH_MICRO_PER_RANK + DEPTH_MICRO_PER_RANK,
        },
        min_zoom: rule.min_zoom,
    }
}

fn stroke_template_from_waterway_rule(rule: &MapWaterwayRule) -> StrokeTemplate {
    StrokeTemplate {
        sort_rank: clamp_u32_to_i16(rule.sort_rank),
        casing: if rule.casing_width > 0.0 {
            Some(StrokePassStyle {
                color: vec4_to_rgb_hex(rule.casing_color),
                width: rule.casing_width,
                shape_id: rule.casing_shape_id,
                expand_class: EXPAND_CLASS_WATER,
                depth_micro: rule.sort_rank as f32 * DEPTH_MICRO_PER_RANK,
            })
        } else {
            None
        },
        center: StrokePassStyle {
            color: vec4_to_rgb_hex(rule.center_color),
            width: rule.center_width,
            shape_id: rule.center_shape_id,
            expand_class: EXPAND_CLASS_WATER,
            depth_micro: rule.sort_rank as f32 * DEPTH_MICRO_PER_RANK + DEPTH_MICRO_PER_RANK,
        },
        min_zoom: rule.min_zoom,
    }
}

fn stroke_template_from_rail_rule(rule: &MapRailRule) -> StrokeTemplate {
    StrokeTemplate {
        sort_rank: clamp_u32_to_i16(rule.sort_rank),
        casing: if rule.casing_width > 0.0 {
            Some(StrokePassStyle {
                color: vec4_to_rgb_hex(rule.casing_color),
                width: rule.casing_width,
                shape_id: rule.casing_shape_id,
                expand_class: EXPAND_CLASS_THIN,
                depth_micro: rule.sort_rank as f32 * DEPTH_MICRO_PER_RANK,
            })
        } else {
            None
        },
        center: StrokePassStyle {
            color: vec4_to_rgb_hex(rule.center_color),
            width: rule.center_width,
            shape_id: rule.center_shape_id,
            expand_class: EXPAND_CLASS_THIN,
            depth_micro: rule.sort_rank as f32 * DEPTH_MICRO_PER_RANK + DEPTH_MICRO_PER_RANK,
        },
        min_zoom: 0.0,
    }
}

fn is_default_key(value: &str) -> bool {
    matches!(value, "" | "*" | "default")
}

fn clamp_u32_to_i16(value: u32) -> i16 {
    value.min(i16::MAX as u32) as i16
}

fn vec4_to_rgb_hex(color: Vec4f) -> u32 {
    color.to_u32() >> 8
}

impl CompiledMapTheme {
    pub fn building_fill_color(&self) -> Option<u32> {
        self.building_fill
    }
}

/// Procedural fill texture, carto-style: 30 = staggered dot stipple
/// (courtyard gardens), 31 = diagonal hatch (playgrounds), 32 = staggered
/// open circles (woods/forests/cemeteries — tree rings). 0 = solid.
pub fn fill_pattern_shape(tags: &HashMap<String, String>) -> f32 {
    match tags.get("leisure").map(|value| value.as_str()) {
        Some("garden") => return 30.0,
        Some("playground") => return 31.0,
        _ => {}
    }
    if matches!(
        tags.get("landuse").map(|value| value.as_str()),
        Some("cemetery" | "forest")
    ) || tags.get("natural").map(|value| value.as_str()) == Some("wood")
    {
        return 32.0;
    }
    0.0
}

pub fn fill_color_for_tags(
    theme: &CompiledMapTheme,
    tags: &HashMap<String, String>,
    closed: bool,
) -> Option<u32> {
    if !closed {
        return None;
    }

    if tags.contains_key("building") {
        return theme.building_fill;
    }
    let layer = tags.get("layer").map(|value| value.as_str()).unwrap_or("");
    if is_road_polygon_layer(layer) {
        return theme.street_area_fill;
    }
    if matches!(layer, "bridges" | "pier_polygons" | "dam_polygons") {
        return theme.bridge_area_fill;
    }
    // Station platforms: carto draws them as calm gray slabs.
    if layer == "platforms" {
        return theme.bridge_area_fill;
    }
    if tag_is(tags, "natural", "water") || tag_is(tags, "waterway", "riverbank") {
        return theme.water_fill;
    }
    if matches!(
        tags.get("natural").map(|value| value.as_str()),
        Some("scrub" | "heath" | "shrubbery")
    ) {
        // carto scrub green; the detail archive routes these as natural=*.
        return theme.landuse_fills.get("grass").copied().or(theme.landuse_default);
    }
    if matches!(
        tags.get("natural").map(|value| value.as_str()),
        Some("sand" | "beach" | "shingle")
    ) {
        // zoo enclosures, dunes, riverbanks — carto's pale sand tan.
        return theme.landuse_fills.get("sand").copied().or(theme.landuse_default);
    }
    if let Some(landuse) = tags.get("landuse") {
        let key = landuse.trim().to_ascii_lowercase();
        if let Some(color) = theme.landuse_fills.get(&key) {
            return Some(*color);
        }
        return theme.landuse_default;
    }
    if let Some(leisure) = tags.get("leisure") {
        let key = leisure.trim().to_ascii_lowercase();
        if let Some(color) = theme.leisure_fills.get(&key) {
            return Some(*color);
        }
        return theme.leisure_default;
    }
    None
}

/// Semantic paint order for the fill pass (carto-like): land/landcover as the
/// base, sites above land, water above both, then buildings, then road areas.
/// Raw MVT layer order painted `land`/`sites` over the buildings.
pub fn fill_layer_rank(tags: &HashMap<String, String>) -> u8 {
    if tags.contains_key("building") {
        return 40;
    }
    let layer = tags.get("layer").map(|value| value.as_str()).unwrap_or("");
    if is_road_polygon_layer(layer) {
        // above base land, but below sites/parks — pedestrian plazas like
        // Bellamyplein must not paint over the park inside them
        return 12;
    }
    if layer == "platforms" {
        return 13;
    }
    // Sand-floored enclosures paint over the zoo's grass.
    if layer == "attraction_area" {
        return 20;
    }
    // Green areas (parks/gardens/grass) rank above generic landuse and sites:
    // they share the `land` layer with huge residential polygons and would
    // otherwise lose to protobuf feature order (Bellamyplein rendered gray).
    let is_green = tags.contains_key("leisure")
        || matches!(
            tags.get("landuse").map(|value| value.as_str()),
            Some(
                "grass"
                    | "forest"
                    | "meadow"
                    | "farmland"
                    | "allotments"
                    | "village_green"
                    | "recreation_ground"
                    | "cemetery"
            )
        );
    match layer {
        "ocean" => 5,
        "land" | "landuse" | "landcover" | "detail_land" => {
            if is_green {
                // Distinct sub-ranks: nested greens (grass patches or a
                // playground inside a park) must never tie — equal ranks
                // shimmer in the tilt-mode micro-depth.
                if let Some(leisure) = tags.get("leisure") {
                    match leisure.as_str() {
                        "park" | "nature_reserve" => 16,
                        "garden" | "golf_course" => 17,
                        "pitch" | "playground" => 19,
                        _ => 16,
                    }
                } else {
                    match tags.get("landuse").map(|value| value.as_str()) {
                        Some("grass") => 18,
                        _ => 17,
                    }
                }
            } else {
                10
            }
        }
        "sites" | "park" | "pois" => 15,
        "water" | "water_polygons" | "water_polygons_labels" => 20,
        "dam_polygons" | "pier_polygons" => 25,
        "bridges" => 45,
        _ => {
            if tag_is(tags, "natural", "water") || tag_is(tags, "waterway", "riverbank") {
                20
            } else if is_green {
                16
            } else {
                10
            }
        }
    }
}

/// Carto-derived road width multiplier per render zoom, relative to the z14
/// baseline widths in the theme (openstreetmap-carto: residential 3px@z14,
/// 5@z15, 6@z16, 12@z17; majors 6@z14 -> 18@z17).
pub fn zoom_width_mult(render_zoom: u32) -> f32 {
    match render_zoom {
        0..=11 => 0.35,
        12 => 0.45,
        13 => 0.7,
        14 => 1.0,
        15 => 1.7,
        16 => 2.1,
        17 => 3.6,
        18 => 4.2,
        _ => 5.0,
    }
}

fn scaled_style(template: StrokeTemplate, rank_bias: i16, width_scale: f32) -> StrokeStyle {
    let rank = (template.sort_rank as i32 + rank_bias as i32)
        .clamp(i16::MIN as i32, i16::MAX as i32) as i16;
    StrokeStyle {
        sort_rank: rank,
        casing: template.casing.map(|casing| StrokePassStyle {
            width: casing.width * width_scale,
            ..casing
        }),
        center: StrokePassStyle {
            width: template.center.width * width_scale,
            ..template.center
        },
    }
}

pub fn stroke_style_for_tags(
    theme: &CompiledMapTheme,
    tags: &HashMap<String, String>,
    tile_zoom: u32,
    render_zoom: u32,
    zoom_mult: f32,
    px_to_units: f32,
) -> Option<StrokeStyle> {
    let layer = tags.get("layer").map(|value| value.as_str()).unwrap_or("");
    if is_road_polygon_layer(layer) || layer == "bridges" {
        // Pedestrian squares / wide path areas get carto's thin gray edge
        // from street level; the fill itself comes from the fill pass.
        if is_road_polygon_layer(layer) && render_zoom >= 15 {
            return Some(StrokeStyle {
                sort_rank: 135,
                casing: None,
                center: StrokePassStyle {
                    color: 0xc2bfba,
                    width: 0.8 * px_to_units,
                    shape_id: 0.0,
                    expand_class: EXPAND_CLASS_CONST_PX,
                    depth_micro: 135.0 * DEPTH_MICRO_PER_RANK,
                },
            });
        }
        return None;
    }

    if matches!(
        layer,
        "street_labels"
            | "street_labels_points"
            | "streets_polygons_labels"
            | "transportation_name"
            | "water_lines_labels"
            | "water_polygons_labels"
            | "boundary_labels"
            | "place_labels"
    ) {
        if !(layer == "street_labels" && tile_zoom < 14) {
            return None;
        }
    }

    if matches!(layer, "street_labels_points" | "streets_polygons_labels") {
        return None;
    }
    // Water label layers carry name geometry only; the water_lines /
    // water_polygons layers own the visible geometry.
    if matches!(layer, "water_lines_labels" | "water_polygons_labels") {
        return None;
    }
    // Walls/fences dark thin, hedges green (detail archive).
    if layer == "barrier_line" {
        let (color, width) = match tags.get("barrier").map(|value| value.as_str()) {
            Some("hedge") => (0x9dc29a, 1.6),
            Some("fence") => (0xaaaaaa, 0.7),
            _ => (0x8a8a8a, 1.0),
        };
        return Some(StrokeStyle {
            sort_rank: 155,
            casing: None,
            center: StrokePassStyle {
                color,
                width: width * px_to_units,
                shape_id: 0.0,
                expand_class: EXPAND_CLASS_CONST_PX,
                depth_micro: 155.0 * DEPTH_MICRO_PER_RANK,
            },
        });
    }
    // Zoo / theme park perimeter: carto's muted purple boundary.
    if layer == "tourism_boundary" {
        return Some(StrokeStyle {
            sort_rank: 150,
            casing: None,
            center: StrokePassStyle {
                color: 0xa383a3,
                width: 1.6 * px_to_units,
                shape_id: 0.0,
                expand_class: EXPAND_CLASS_CONST_PX,
                depth_micro: 150.0 * DEPTH_MICRO_PER_RANK,
            },
        });
    }
    // Platform slabs get a thin constant-px edge like carto.
    if layer == "platforms" {
        return Some(StrokeStyle {
            sort_rank: 140,
            casing: None,
            center: StrokePassStyle {
                color: 0x9a938b,
                width: 0.8 * px_to_units,
                shape_id: 0.0,
                expand_class: EXPAND_CLASS_CONST_PX,
                depth_micro: 140.0 * DEPTH_MICRO_PER_RANK,
            },
        });
    }

    let mut width_scale = zoom_mult * px_to_units;
    let mut rank_bias = 0_i16;

    if tag_is_truthy(tags, "link") {
        width_scale *= 0.7;
        rank_bias -= 10;
    }
    if tag_is_truthy(tags, "tunnel") {
        rank_bias -= 22;
    }
    if tag_is_truthy(tags, "bridge") {
        rank_bias += 26;
    }

    if let Some(highway) = tags.get("highway") {
        let key = highway.trim().to_ascii_lowercase();
        // shortbread carries rail/tram lines inside the streets layer; don't
        // let them fall through to the generic road style
        if tag_is_truthy(tags, "rail")
            || matches!(
                key.as_str(),
                "rail" | "tram" | "light_rail" | "subway" | "narrow_gauge" | "funicular"
                    | "monorail"
            )
        {
            let template = theme.railway_rule?;
            let rail_scale = zoom_mult.max(1.0).powf(0.35) * px_to_units;
            // shortbread sets rail=true on ALL railways including trams;
            // the kind decides tram/metro vs heavy rail.
            let heavy = matches!(
                key.as_str(),
                "rail" | "narrow_gauge" | "funicular" | "monorail"
            ) || (tag_is_truthy(tags, "rail")
                && !matches!(key.as_str(), "tram" | "light_rail" | "subway"));
            return Some(rail_stroke_style(
                template,
                heavy,
                render_zoom,
                tags,
                rank_bias,
                rail_scale,
            ));
        }
        let template = theme.road_rules.get(&key).copied().or(theme.road_default)?;
        // Carto hides footways/paths well before roads; clutter at city
        // scale otherwise (salmon dot confetti over every block).
        if (render_zoom as f32) < template.min_zoom {
            return None;
        }
        // Paths/footways/cycleways (thin, uncased) barely grow with zoom in
        // carto (~1px at z15 and z17 alike) while regular roads grow steeply.
        let mut thin_growth = false;
        if template.casing.is_none() && template.center.width <= 2.0 {
            width_scale = zoom_mult.max(1.0).powf(0.35) * px_to_units;
            if tag_is_truthy(tags, "link") {
                width_scale *= 0.7;
            }
            thin_growth = true;
        }
        let mut style = scaled_style(template, rank_bias, width_scale);
        if thin_growth {
            style.center.expand_class = EXPAND_CLASS_THIN;
            // carto grays out paths you can't freely walk (zoos, private
            // grounds) instead of the public salmon.
            if matches!(
                tags.get("access").map(|value| value.as_str()),
                Some("private" | "no" | "customers")
            ) {
                style.center.color = 0x9c9c9c;
            }
            // Footway bridges get a small white deck under the dots (the
            // "outline box" carto draws over water crossings).
            if tag_is_truthy(tags, "bridge") {
                style.casing = Some(StrokePassStyle {
                    color: 0xffffff,
                    width: style.center.width * 2.4,
                    shape_id: 0.0,
                    expand_class: EXPAND_CLASS_THIN,
                    depth_micro: style.center.depth_micro - DEPTH_MICRO_PER_RANK,
                });
            }
        }
        // Bridges float above (and tunnels below) their base rank in the
        // tilt-mode micro-depth as well, so crossings resolve stably.
        if rank_bias != 0 {
            let micro_bias = rank_bias as f32 * DEPTH_MICRO_PER_RANK;
            style.center.depth_micro += micro_bias;
            if let Some(casing) = style.casing.as_mut() {
                casing.depth_micro += micro_bias;
            }
        }
        // carto draws bridge ROADS with a dark casing edge — never thin
        // footpaths/steps: their sub-px dark casing under the salmon dots
        // reads as black dashed fragments at every canal crossing.
        if tag_is_truthy(tags, "bridge") && !thin_growth {
            let width = style
                .casing
                .map_or(style.center.width * 1.35, |casing| casing.width);
            style.casing = Some(StrokePassStyle {
                color: 0x4a4a4a,
                width,
                shape_id: 0.0,
                expand_class: EXPAND_CLASS_ROAD,
                // One step under the center: a tie would let the dark edge
                // noise-win over the road fill (black bridges).
                depth_micro: style.center.depth_micro - DEPTH_MICRO_PER_RANK,
            });
        }
        if tag_is_truthy(tags, "tunnel") {
            style.center.shape_id = 11.0;
            if let Some(casing) = style.casing.as_mut() {
                casing.shape_id = 11.0;
            }
        }
        return Some(style);
    }

    if let Some(waterway) = tags.get("waterway") {
        let key = waterway.trim().to_ascii_lowercase();
        let template = theme
            .waterway_rules
            .get(&key)
            .copied()
            .or(theme.waterway_default)?;
        if (render_zoom as f32) < template.min_zoom {
            return None;
        }
        // Waterway centerlines shrink faster than roads below z14 — carto
        // keeps canals at ~1px at z12/z13, letting the water polygons carry
        // the visual weight; linear scaling read as "the city is all water".
        let water_scale = if zoom_mult < 1.0 {
            zoom_mult.powf(1.6) * px_to_units
        } else {
            width_scale
        };
        return Some(scaled_style(template, rank_bias, water_scale));
    }

    if let Some(railway) = tags.get("railway") {
        let key = railway.trim().to_ascii_lowercase();
        // Non-track railway features (platforms, stations, dead lines) are
        // not linework.
        if matches!(
            key.as_str(),
            "platform" | "station" | "razed" | "abandoned" | "proposed" | "disused"
        ) {
            return None;
        }
        let template = theme.railway_rule?;
        let rail_scale = zoom_mult.max(1.0).powf(0.35) * px_to_units;
        let heavy = matches!(key.as_str(), "rail" | "narrow_gauge" | "funicular" | "monorail");
        return Some(rail_stroke_style(
            template,
            heavy,
            render_zoom,
            tags,
            rank_bias,
            rail_scale,
        ));
    }

    None
}

/// Zoom from which heavy rail draws the carto sleeper look (solid dark
/// casing + even light dash core); below it, and for tram/metro always,
/// rails stay the theme's single thin line. z14 and out, parallel station
/// tracks merge into a striped blob — stay thin there.
const RAIL_SLEEPER_MIN_ZOOM: u32 = 15;
/// Trams darken toward carto's near-black only at street level; at city
/// scale they stay the theme's faint gray so they don't dominate.
const TRAM_DARK_MIN_ZOOM: u32 = 16;

fn rail_stroke_style(
    template: StrokeTemplate,
    heavy: bool,
    render_zoom: u32,
    tags: &HashMap<String, String>,
    rank_bias: i16,
    width_scale: f32,
) -> StrokeStyle {
    let mut template = template;
    if !heavy && render_zoom >= TRAM_DARK_MIN_ZOOM {
        // Trams/metro draw as a solid near-black line at street zooms in
        // carto. Only darken light-theme grays; the dark palette stays.
        let c = template.center.color;
        let avg = ((c >> 16 & 0xff) + (c >> 8 & 0xff) + (c & 0xff)) / 3;
        if avg < 0x90 {
            template.center.color = 0x505050;
        }
        template.center.width = template.center.width.max(1.2);
    }
    if heavy && render_zoom >= RAIL_SLEEPER_MIN_ZOOM && !tag_is_truthy(tags, "tunnel") {
        let rank = template.sort_rank as f32;
        // Sidings/yards/spurs (dead-end service tracks) fade toward the
        // background like carto's lighter service rail.
        let service = tags.contains_key("service");
        let casing_color = if service {
            let c = template.center.color;
            let lighten = |v: u32| v + (0xff - v) * 45 / 100;
            (lighten(c >> 16 & 0xff) << 16) | (lighten(c >> 8 & 0xff) << 8) | lighten(c & 0xff)
        } else {
            template.center.color
        };
        template.casing = Some(StrokePassStyle {
            // The theme's rail line color becomes the casing band.
            color: casing_color,
            width: if service { 2.0 } else { 2.4 },
            shape_id: 0.0,
            expand_class: EXPAND_CLASS_THIN,
            depth_micro: rank * DEPTH_MICRO_PER_RANK,
        });
        template.center = StrokePassStyle {
            color: 0xf7f7f7,
            width: if service { 1.0 } else { 1.2 },
            shape_id: 12.0,
            expand_class: EXPAND_CLASS_THIN,
            depth_micro: rank * DEPTH_MICRO_PER_RANK + DEPTH_MICRO_PER_RANK,
        };
    }
    scaled_style(template, rank_bias, width_scale)
}
